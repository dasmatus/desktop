//! A run, and what Settings reads about the last one.
//!
//! The timer starts a run every hour, and a run started by it goes ahead
//! only when the backups are on and the last finished one is a day old:
//! so a computer that was off, or away from the box, catches up within the
//! hour after it is back, without a run every hour. A run that cannot
//! reach the box is skipped, not failed, and the next hour tries again.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{
    boxes::{self, Missing, Route},
    cloud::{self, Cloud},
    config::{self, Account, Config, Paths},
    forge::{Forge, Names},
    host_name, now,
    walk::{self, Tree},
    Error,
};

/// How old the last finished backup may get before the timer backs up.
pub const INTERVAL: u64 = 24 * 60 * 60;

/// How many files go up at once.
pub const UPLOADERS: usize = 4;

/// How many failures a report names; the rest are counted.
pub const NAMED_FAILURES: usize = 20;

/// Failures to reach the box in a row after which a run stops.
pub const UNREACHABLE_STOP: usize = 8;

/// How a run ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// It went through; some files may still have been refused.
    #[default]
    Done,
    /// It stopped partway.
    Failed,
    /// It could not reach the box, so did nothing.
    Skipped,
}

/// What a run did, `last.json`'s `last`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Report {
    /// When it started and finished, in seconds since the epoch.
    pub started: u64,
    /// See `started`.
    pub finished: u64,
    /// The box it was for.
    #[serde(rename = "box")]
    pub box_name: String,
    /// The LosOS boxes machine it went through, or empty.
    pub machine: String,
    /// How it ended.
    pub outcome: Outcome,
    /// Why it failed or was skipped.
    pub message: String,
    /// Files uploaded to LosOS cloud, and their bytes.
    pub uploaded: u64,
    /// See `uploaded`.
    pub bytes: u64,
    /// Files and folders deleted from LosOS cloud.
    pub deleted: u64,
    /// Files already up to date.
    pub unchanged: u64,
    /// Files that could not be uploaded or read.
    pub failed: u64,
    /// The first of those, each with why.
    pub failures: Vec<String>,
    /// Repositories pushed to LosOS Git.
    pub repos: u64,
    /// Config files in LosOS Git's configs repository after the run, or 0
    /// when they went to LosOS cloud with the rest.
    pub configs: u64,
    /// Whether the run committed a change to them.
    pub configs_changed: bool,
    /// Repositories that could not be, each with why.
    pub repo_failures: Vec<String>,
}

impl Report {
    fn fail_file(&mut self, what: String) {
        self.failed += 1;
        if self.failures.len() < NAMED_FAILURES {
            self.failures.push(what);
        }
    }
}

/// The last run, and when one last finished: `last.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    /// The last run's report.
    pub last: Option<Report>,
    /// When the last run that went through finished.
    pub last_done: u64,
}

/// What is in LosOS cloud: `files.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Index {
    /// The box, account and computer it is for; another starts it over.
    pub owner: String,
    /// Each file's path, with the size and time it had when uploaded.
    pub files: BTreeMap<String, (u64, i64)>,
    /// The folders made.
    pub dirs: BTreeSet<String>,
}

/// Whether the timer's run should back up now.
pub fn due(config: &Config, history: &History, now: u64) -> bool {
    // Ten minutes early, so a daily run does not slip an hour a day.
    config.enabled && config.ready() && now + 600 >= history.last_done + INTERVAL
}

/// Where the relay's hosts files are; `LOSOS_BACKUP_HOSTS_DIR` moves it
/// for tests.
pub fn hosts_dir() -> std::path::PathBuf {
    std::env::var_os("LOSOS_BACKUP_HOSTS_DIR").map_or_else(|| boxes::HOSTS_DIR.into(), Into::into)
}

/// Runs a backup, or with `if_due`, one when [`due`]. Returns its report,
/// or `None` when nothing ran: not due, or a run already going.
pub fn run(paths: &Paths, if_due: bool) -> Result<Option<Report>, Error> {
    fs::create_dir_all(&paths.state)?;
    let lock = fs::File::create(paths.lock_file())?;
    if lock.try_lock().is_err() {
        info!("a backup is running already");
        return Ok(None);
    }
    let config = paths.load_config()?;
    let mut history: History = config::load(&paths.report_file())?;
    if if_due && !due(&config, &history, now()) {
        return Ok(None);
    }
    if !config.ready() {
        return Err(Error::new(
            "Choose a LosOS box and sign in to LosOS cloud or LosOS Git first.",
        ));
    }
    let mut report = Report {
        started: now(),
        box_name: config.box_name.clone(),
        ..Report::default()
    };
    if let Err(e) = reach_and_back_up(paths, &config, &mut report) {
        warn!(error = %e, "backup stopped");
        report.outcome = Outcome::Failed;
        report.message = e.message;
    }
    report.finished = now();
    if report.outcome == Outcome::Done {
        history.last_done = report.finished;
    }
    history.last = Some(report.clone());
    config::save(&paths.report_file(), &history, false)?;
    info!(
        outcome = ?report.outcome,
        uploaded = report.uploaded,
        deleted = report.deleted,
        failed = report.failed,
        repos = report.repos,
        "backup finished"
    );
    Ok(Some(report))
}

fn reach_and_back_up(paths: &Paths, config: &Config, report: &mut Report) -> Result<(), Error> {
    let hosts = hosts_dir();
    let (route, started) = match boxes::find(&config.box_name, &hosts) {
        Ok(route) => (route, None),
        Err(Missing::NotRouted) => match boxes::start_machine_for(&config.box_name, &hosts) {
            Ok((route, machine)) => (route, Some(machine)),
            Err(e) => {
                report.outcome = Outcome::Skipped;
                report.message = e.message;
                return Ok(());
            }
        },
        Err(Missing::NotFound(why)) => {
            info!(%why, "the box is not here");
            report.outcome = Outcome::Skipped;
            report.message = format!("{} is not on this network.", config.box_name);
            return Ok(());
        }
    };
    report.machine = route.machine.clone().unwrap_or_default();
    let result = back_up(paths, config, &route, report);
    if let Some(machine) = started {
        if let Err(e) = boxes::pm_machine("stop", &machine) {
            warn!(error = %e, "cannot stop the LosOS boxes machine again");
        }
    }
    result
}

fn back_up(
    paths: &Paths,
    config: &Config,
    route: &Route,
    report: &mut Report,
) -> Result<(), Error> {
    let pem = fs::read_to_string(paths.cert_file())
        .map_err(|_| Error::new("The box's certificate is missing: choose the box again."))?;
    let cert = boxes::pem_to_der(&pem)
        .ok_or_else(|| Error::new("The box's certificate is damaged: choose the box again."))?;
    let agent = boxes::pinned(route, &cert)?;
    let secrets = paths.load_secrets()?;
    let host = host_name();
    let git = config.git.as_ref().zip(secrets.git.as_deref());
    let cloud = config.cloud.as_ref().zip(secrets.cloud.as_deref());
    let tree = walk::walk(
        &paths.home,
        &walk::Options {
            excluded: walk::read_excluded(&paths.exclude_file()),
            repos_as_files: git.is_none(),
        },
    );
    for path in &tree.unreadable {
        report.fail_file(format!(
            "~/{path}: cannot be read, or its name is not UTF-8"
        ));
    }
    let mut tree = tree;
    let cert_file = paths.cert_file();
    let mut forge = None;
    let mut stopped = None;
    // The configs first, to their repository in LosOS Git: only once they
    // are there do they leave LosOS cloud's share, so a failed push leaves
    // them with the rest rather than nowhere.
    if let Some((account, token)) = git {
        let f = Forge::new(&agent, route, &account.user, token, &cert_file);
        let owner = format!("{}/{}", route.name, account.user);
        let mut names: Names = config::load(&paths.repos_file())?;
        if names.owner != owner {
            names = Names {
                owner,
                ..Names::default()
            };
        }
        let configs: Vec<&str> = tree
            .files
            .iter()
            .map(|f| f.rel.as_str())
            .filter(|rel| walk::is_config(rel))
            .collect();
        let name = names.config(&host);
        match f.push_files(
            &paths.home,
            &configs,
            &name,
            &host,
            &paths.state.join("config.git"),
        ) {
            Ok(changed) => {
                report.configs = tree.take_configs().len() as u64;
                report.configs_changed = changed;
            }
            Err(e) => {
                report.repo_failures.push(format!("the configs: {e}"));
                if e.unreachable {
                    stopped = Some(e);
                }
            }
        }
        forge = Some((f, names));
    }
    // Files next, then repositories, each going as far as it can: a
    // refused sign-in to one says nothing about the other.
    if let Some((account, password)) = cloud {
        if stopped.is_none() {
            if let Err(e) = upload(
                paths, &agent, route, account, password, &host, &tree, report,
            ) {
                stopped = Some(e);
            }
        }
    }
    if let Some((forge, mut names)) = forge {
        if stopped.as_ref().is_none_or(|e| !e.unreachable) {
            for repo in &tree.repos {
                let name = names.name(&repo.rel);
                match forge.push(repo, &name, &host, &paths.state.join("git")) {
                    Ok(pushed) => report.repos += u64::from(pushed),
                    Err(e) => {
                        report.repo_failures.push(format!("~/{}: {e}", repo.rel));
                        if e.unreachable {
                            stopped.get_or_insert(e);
                            break;
                        }
                    }
                }
            }
        }
        config::save(&paths.repos_file(), &names, false)?;
    }
    stopped.map_or(Ok(()), Err)
}

/// Brings LosOS cloud's copy of the home in step with `tree`.
#[allow(clippy::too_many_arguments)]
fn upload(
    paths: &Paths,
    agent: &ureq::Agent,
    route: &Route,
    account: &Account,
    password: &str,
    host: &str,
    tree: &Tree,
    report: &mut Report,
) -> Result<(), Error> {
    let cloud = Cloud::new(
        agent,
        cloud::base(&route.name),
        &account.login,
        &account.user,
        password,
        host,
    );
    let owner = format!("{}/{}/{host}", route.name, account.user);
    let mut index: Index = config::load(&paths.index_file())?;
    if index.owner != owner {
        index = Index {
            owner,
            ..Index::default()
        };
    }
    cloud.prepare()?;

    let new_dirs: Vec<&String> = tree
        .dirs
        .iter()
        .filter(|d| !index.dirs.contains(*d))
        .collect();
    for dir in new_dirs {
        match cloud.mkcol(dir) {
            Ok(()) => {
                index.dirs.insert(dir.clone());
            }
            Err(e) if e.unreachable => return Err(e),
            Err(e) => report.fail_file(format!("~/{dir}: {e}")),
        }
    }

    let changed: Vec<&walk::File> = tree
        .files
        .iter()
        .filter(|f| index.files.get(&f.rel) != Some(&(f.size, f.mtime)))
        .collect();
    report.unchanged = (tree.files.len() - changed.len()) as u64;
    info!(
        changed = changed.len(),
        unchanged = report.unchanged,
        "uploading"
    );
    let stop = AtomicBool::new(false);
    let mut unreachable = None;
    let mut saved = Instant::now();
    std::thread::scope(|scope| {
        let (work, todo) = crossbeam_channel::bounded::<&walk::File>(UPLOADERS * 4);
        let (done, results) = crossbeam_channel::unbounded();
        for _ in 0..UPLOADERS {
            let (todo, done, cloud) = (todo.clone(), done.clone(), &cloud);
            scope.spawn(move || {
                for file in todo {
                    let result = cloud.put(
                        &file.rel,
                        &paths.home.join(&file.rel),
                        file.size,
                        file.mtime.div_euclid(1_000_000_000),
                    );
                    if done.send((file, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop((todo, done));
        let stop = &stop;
        scope.spawn(move || {
            for file in changed {
                if stop.load(Ordering::Relaxed) || work.send(file).is_err() {
                    break;
                }
            }
        });
        let mut in_a_row = 0;
        for (file, result) in results {
            match result {
                Ok(()) => {
                    in_a_row = 0;
                    report.uploaded += 1;
                    report.bytes += file.size;
                    index
                        .files
                        .insert(file.rel.clone(), (file.size, file.mtime));
                }
                Err(e) if e.unreachable => {
                    in_a_row += 1;
                    report.fail_file(format!("~/{}: {e}", file.rel));
                    if in_a_row >= UNREACHABLE_STOP {
                        stop.store(true, Ordering::Relaxed);
                        unreachable.get_or_insert(e);
                    }
                }
                Err(e) => report.fail_file(format!("~/{}: {e}", file.rel)),
            }
            // Kept as it goes, so a first backup that stops partway picks
            // up where it was.
            if saved.elapsed() > Duration::from_secs(30) {
                if let Err(e) = config::save(&paths.index_file(), &index, false) {
                    warn!(error = %e, "cannot keep the index");
                }
                saved = Instant::now();
            }
        }
    });
    if let Some(e) = unreachable {
        config::save(&paths.index_file(), &index, false)?;
        return Err(e);
    }

    let result = delete_gone(&cloud, tree, &mut index, report);
    config::save(&paths.index_file(), &index, false)?;
    result
}

/// Deletes from LosOS cloud what is no longer in the home: each folder
/// gone with everything in it, then each file gone from a folder that is
/// still there. Nothing under a path the walk could not read, and nothing
/// at all when the home came up empty, which is a home not there rather
/// than one emptied.
fn delete_gone(
    cloud: &Cloud<'_>,
    tree: &Tree,
    index: &mut Index,
    report: &mut Report,
) -> Result<(), Error> {
    if tree.files.is_empty() && tree.dirs.is_empty() && !index.files.is_empty() {
        warn!("the home is empty: deleting nothing");
        return Ok(());
    }
    let under = |path: &str, dir: &str| {
        path.starts_with(dir) && path.as_bytes().get(dir.len()) == Some(&b'/')
    };
    let kept = |path: &str| {
        tree.unreadable
            .iter()
            .any(|u| u.is_empty() || path == u || under(path, u))
    };
    let dirs: BTreeSet<&str> = tree.dirs.iter().map(String::as_str).collect();
    let files: BTreeSet<&str> = tree.files.iter().map(|f| f.rel.as_str()).collect();
    let gone_dirs: Vec<String> = index
        .dirs
        .iter()
        .filter(|d| !dirs.contains(d.as_str()) && !kept(d))
        .cloned()
        .collect();
    let mut deleted_dirs: Vec<String> = Vec::new();
    for dir in gone_dirs {
        if deleted_dirs.iter().any(|d| under(&dir, d)) {
            continue;
        }
        match cloud.delete(&dir) {
            Ok(()) => {
                report.deleted += 1;
                index.files.retain(|f, _| !under(f, &dir));
                index.dirs.retain(|d| *d != dir && !under(d, &dir));
                deleted_dirs.push(dir);
            }
            Err(e) if e.unreachable => return Err(e),
            Err(e) => report.fail_file(format!("~/{dir}: {e}")),
        }
    }
    let gone_files: Vec<String> = index
        .files
        .keys()
        .filter(|f| !files.contains(f.as_str()) && !kept(f))
        .cloned()
        .collect();
    for file in gone_files {
        match cloud.delete(&file) {
            Ok(()) => {
                report.deleted += 1;
                index.files.remove(&file);
            }
            Err(e) if e.unreachable => return Err(e),
            Err(e) => report.fail_file(format!("~/{file}: {e}")),
        }
    }
    Ok(())
}

/// What the Backups page shows: `losos-backup status`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    /// The box chosen, or empty.
    #[serde(rename = "box")]
    pub box_name: String,
    /// Its certificate's fingerprint.
    pub fingerprint: String,
    /// Whether the timer backs up.
    pub enabled: bool,
    /// The LosOS cloud account signed in to, by login.
    pub cloud: Option<String>,
    /// The LosOS Git account signed in to, by login.
    pub git: Option<String>,
    /// Whether a run is going now.
    pub running: bool,
    /// The last run.
    pub last: Option<Report>,
    /// When the last run that went through finished, or 0.
    pub last_done: u64,
    /// Whether this computer reaches boxes through a LosOS boxes machine.
    pub routed: bool,
    /// The LosOS boxes machines it has.
    pub machines: Vec<String>,
    /// The boxes it can choose from now.
    pub boxes: Vec<String>,
}

/// The status, as far as it can be read.
pub fn status(paths: &Paths) -> Status {
    let config = paths.load_config().unwrap_or_default();
    let history: History = config::load(&paths.report_file()).unwrap_or_default();
    let running = fs::File::open(paths.lock_file()).is_ok_and(|lock| lock.try_lock().is_err());
    let hosts = hosts_dir();
    let routed = boxes::routed(&hosts).is_some();
    Status {
        box_name: config.box_name,
        fingerprint: config.fingerprint,
        enabled: config.enabled,
        cloud: config.cloud.map(|a| a.login),
        git: config.git.map(|a| a.login),
        running,
        last: history.last,
        last_done: history.last_done,
        routed,
        machines: if routed {
            boxes::machines()
        } else {
            Vec::new()
        },
        boxes: boxes::offered(&hosts),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_due_a_day_after_the_last_backup() {
        let mut config = Config {
            box_name: "mattbox.local".into(),
            enabled: true,
            git: Some(Account::default()),
            ..Config::default()
        };
        let mut history = History::default();
        assert!(due(&config, &history, 1_000_000));
        history.last_done = 1_000_000;
        assert!(!due(&config, &history, 1_000_000 + 3600));
        assert!(due(&config, &history, 1_000_000 + INTERVAL - 300));
        assert!(due(&config, &history, 1_000_000 + 5 * INTERVAL));
        config.enabled = false;
        assert!(!due(&config, &history, 1_000_000 + 5 * INTERVAL));
    }
}
