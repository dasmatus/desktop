use std::fs;
use std::path::PathBuf;

use losos_installer::disks::{self, Disk};
use losos_installer::github;
use losos_installer::install;
use losos_installer::serve::{self, Request};
use losos_installer::stage;

/// A scratch directory per test, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("losos-installer-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn write(&self, path: &str, contents: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fake_disk(sys: &Scratch, name: &str, sectors: u64, model: &str) {
    sys.write(&format!("block/{name}/size"), &format!("{sectors}\n"));
    sys.write(
        &format!("block/{name}/device/model"),
        &format!("{model}   \n"),
    );
    sys.write(&format!("block/{name}/removable"), "0\n");
    sys.write(&format!("block/{name}/ro"), "0\n");
}

#[test]
fn disks_leave_out_what_cannot_be_installed_onto() {
    let sys = Scratch::new("disks");
    fake_disk(&sys, "sda", 1_000_215_216, "Samsung SSD 870");
    fake_disk(&sys, "sdb", 60_063_744, "Flash Disk");
    sys.write("block/sdb/removable", "1\n");
    fake_disk(&sys, "nvme0n1", 2_000_409_264, "WD_BLACK SN850X");
    fake_disk(&sys, "loop0", 2048, "");
    fake_disk(&sys, "sr0", 2048, "DVD");
    fake_disk(&sys, "mmcblk0boot0", 8192, "");
    fake_disk(&sys, "zram0", 8192, "");
    fake_disk(&sys, "sdc", 0, "Card reader");
    fake_disk(&sys, "sdd", 2048, "Write protected");
    sys.write("block/sdd/ro", "1\n");
    fake_disk(&sys, "nvme1c1n1", 2048, "multipath");
    sys.write("block/nvme1c1n1/hidden", "1\n");

    let found = disks::list(&sys.0, Some("sdb"));
    assert_eq!(
        found,
        [
            Disk {
                name: "nvme0n1".into(),
                path: "/dev/nvme0n1".into(),
                size: 2_000_409_264 * 512,
                model: "WD_BLACK SN850X".into(),
                removable: false,
            },
            Disk {
                name: "sda".into(),
                path: "/dev/sda".into(),
                size: 1_000_215_216 * 512,
                model: "Samsung SSD 870".into(),
                removable: false,
            },
        ]
    );
    assert!(
        disks::list(&sys.0, None)
            .iter()
            .any(|d| d.name == "sdb" && d.removable)
    );
}

#[test]
fn the_medium_is_found_from_its_mount() {
    let sys = Scratch::new("medium");
    fs::create_dir_all(sys.0.join("block/sdb/sdb1")).unwrap();
    sys.write("block/sdb/sdb1/partition", "1\n");
    fs::create_dir_all(sys.0.join("block/sr0")).unwrap();
    fs::create_dir_all(sys.0.join("dev/block")).unwrap();
    std::os::unix::fs::symlink("../../block/sdb/sdb1", sys.0.join("dev/block/8:17")).unwrap();
    std::os::unix::fs::symlink("../../block/sr0", sys.0.join("dev/block/11:0")).unwrap();

    let stick = "22 1 0:21 / / rw - tmpfs tmpfs rw\n\
        35 22 8:17 / /iso ro,relatime - iso9660 /dev/sdb1 ro\n";
    assert_eq!(
        disks::backing_disk(stick, &sys.0, "/iso").as_deref(),
        Some("sdb")
    );
    let optical = "35 22 11:0 / /iso ro,relatime - iso9660 /dev/sr0 ro\n";
    assert_eq!(
        disks::backing_disk(optical, &sys.0, "/iso").as_deref(),
        Some("sr0")
    );
    assert_eq!(disks::backing_disk(stick, &sys.0, "/elsewhere"), None);
}

#[test]
fn esp_node_from_repart_json() {
    let json = r#"[
        {"type":"esp","label":"esp","uuid":"x","node":"/dev/sda1","activity":"create"},
        {"type":"usr-x86-64-verity","label":"_empty","node":"/dev/sda2"}
    ]"#;
    assert_eq!(install::esp_node(json).unwrap(), PathBuf::from("/dev/sda1"));
    assert!(install::esp_node(r#"[{"type":"usr-x86-64","node":"/dev/sda2"}]"#).is_err());
    assert!(install::esp_node("not json").is_err());
}

#[test]
fn transfers_name_the_chosen_disk() {
    let dir = Scratch::new("transfers");
    dir.write(
        "templates/20-usr.transfer",
        "[Target]\nType=partition\nPath=@TARGET@\n",
    );
    dir.write(
        "templates/30-uki.transfer",
        "[Target]\nType=regular-file\nPath=/run/esp/EFI/Linux\n",
    );
    dir.write("templates/README", "not a transfer @TARGET@\n");
    // A stale file from an earlier attempt must not survive into this one.
    dir.write("out/10-old.transfer", "stale\n");

    install::render_transfers(
        &dir.0.join("templates"),
        &dir.0.join("out"),
        "/dev/nvme0n1",
        install::Source::Channel,
    )
    .unwrap();
    let mut names: Vec<_> = fs::read_dir(dir.0.join("out"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["20-usr.transfer", "30-uki.transfer"]);
    assert_eq!(
        fs::read_to_string(dir.0.join("out/20-usr.transfer")).unwrap(),
        "[Target]\nType=partition\nPath=/dev/nvme0n1\n"
    );
    assert!(
        install::render_transfers(
            &dir.0.join("out/none"),
            &dir.0.join("o2"),
            "/dev/sda",
            install::Source::Channel
        )
        .is_err()
    );
}

#[test]
fn a_release_disk_replaces_only_the_source() {
    let dir = Scratch::new("local");
    dir.write(
        "templates/20-usr.transfer",
        "[Transfer]\nVerify=true\n\n[Source]\nMatchPattern=losos_@v.raw.xz\n\
         Path=https://proxy.example/updates/nightly/x86_64/\nType=url-file\n\n\
         [Target]\nPath=@TARGET@\nType=partition\n",
    );
    let release = dir.0.join("release");
    install::render_transfers(
        &dir.0.join("templates"),
        &dir.0.join("out"),
        "/dev/vda",
        install::Source::Disk(install::Local {
            url: "https://proxy.example/updates/nightly/x86_64/",
            dir: &release,
        }),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(dir.0.join("out/20-usr.transfer")).unwrap(),
        format!(
            "[Transfer]\nVerify=true\n\n[Source]\nMatchPattern=losos_@v.raw.xz\n\
             Path={}\nType=regular-file\n\n[Target]\nPath=/dev/vda\nType=partition\n",
            release.display()
        )
    );
}

#[test]
fn the_github_release_replaces_only_the_url() {
    let dir = Scratch::new("mirror");
    let template = "[Transfer]\nVerify=true\n\n[Source]\nMatchPattern=losos_@v.raw.xz\n\
         Path=https://proxy.example/updates/nightly/x86_64/\nType=url-file\n\n\
         [Target]\nPath=@TARGET@\nType=partition\n";
    dir.write("templates/20-usr.transfer", template);
    install::render_transfers(
        &dir.0.join("templates"),
        &dir.0.join("out"),
        "/dev/vda",
        install::Source::Mirror {
            url: "https://proxy.example/updates/nightly/x86_64/",
            mirror: "http://127.0.0.1:4321/",
        },
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(dir.0.join("out/20-usr.transfer")).unwrap(),
        template
            .replace(
                "https://proxy.example/updates/nightly/x86_64/",
                "http://127.0.0.1:4321/"
            )
            .replace("@TARGET@", "/dev/vda")
    );
}

fn release_json(tag: &str, prerelease: bool, names: &[&str]) -> serde_json::Value {
    let assets: Vec<_> = names
        .iter()
        .map(|n| {
            serde_json::json!({
                "name": n,
                "browser_download_url": format!("https://github.example/{tag}/{n}"),
                "size": 10,
            })
        })
        .collect();
    serde_json::json!({"tag_name": tag, "prerelease": prerelease, "draft": false, "assets": assets})
}

#[test]
fn nightly_tags_come_from_the_version() {
    assert_eq!(
        github::nightly_tag("20261010.175322").as_deref(),
        Some("nightly-20261010T1753")
    );
    assert_eq!(github::nightly_tag("0.1"), None);
    assert_eq!(github::nightly_tag("20261010.17532x"), None);
}

#[test]
fn a_tagged_release_is_picked_before_its_nightly() {
    let v = "20261010.175322";
    let files = [
        "SHA256SUMS-x86_64",
        "SHA256SUMS-x86_64.gpg",
        "losos-desktop_20261010.175322_x86_64.efi",
    ];
    let other = [
        "SHA256SUMS-x86_64",
        "losos-desktop_20261009.000000_x86_64.efi",
    ];
    let only_arm = [
        "SHA256SUMS-aarch64",
        "losos-desktop_20261010.175322_aarch64.efi",
    ];
    let draft = {
        let mut json = release_json("v9", false, &files);
        json["draft"] = true.into();
        json
    };
    assert!(github::parse_release(&draft).is_none());
    let releases: Vec<_> = [
        release_json("nightly-20261010T1753", true, &files),
        release_json("v0.0.1", false, &files),
        release_json("v0.0.0", false, &other),
        release_json("arm", false, &only_arm),
    ]
    .iter()
    .filter_map(github::parse_release)
    .collect();
    assert_eq!(
        github::pick(releases.clone(), v, "x86_64").unwrap().tag,
        "v0.0.1"
    );
    assert_eq!(
        github::pick(releases[..1].to_vec(), v, "x86_64")
            .unwrap()
            .tag,
        "nightly-20261010T1753"
    );
    assert!(github::pick(releases, "20261011.000000", "x86_64").is_none());
}

#[test]
fn split_files_answer_under_their_own_name() {
    let release = github::parse_release(&release_json(
        "t",
        true,
        &[
            "SHA256SUMS-x86_64",
            "SHA256SUMS-x86_64.gpg",
            "SHA256SUMS-aarch64",
            "SHA256SUMS-aarch64.gpg",
            "a_x86_64.efi",
            "a_usr.raw.xz.part-01",
            "a_usr.raw.xz.part-00",
            "a_usr.raw.xz.part-02",
        ],
    ))
    .unwrap();
    let files = github::files(&release, "x86_64");
    let names: Vec<_> = files.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "SHA256SUMS",
            "SHA256SUMS.gpg",
            "a_usr.raw.xz",
            "a_x86_64.efi"
        ]
    );
    assert_eq!(
        files["SHA256SUMS"].urls,
        ["https://github.example/t/SHA256SUMS-x86_64"]
    );
    let usr = &files["a_usr.raw.xz"];
    assert_eq!(usr.size, 30);
    assert_eq!(
        usr.urls,
        [
            "https://github.example/t/a_usr.raw.xz.part-00",
            "https://github.example/t/a_usr.raw.xz.part-01",
            "https://github.example/t/a_usr.raw.xz.part-02",
        ]
    );
}

#[test]
fn sums_parse_in_both_forms() {
    let a = "a".repeat(64);
    let b = "b".repeat(64);
    let text = format!("{a}  one.raw.xz\n{b} *two.efi\nnot a line\n");
    assert_eq!(
        install::parse_sums(&text),
        [(a, "one.raw.xz".to_owned()), (b, "two.efi".to_owned())]
    );
}

#[test]
fn a_release_is_checked_file_by_file() {
    let dir = Scratch::new("verify");
    let (tx, _rx) = std::sync::mpsc::channel();
    let pubring = dir.0.join("no-such-pubring.gpg");
    // sha256("hello\n")
    let hello = "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03";
    dir.write("release/one.efi", "hello\n");
    dir.write("release/SHA256SUMS", &format!("{hello}  one.efi\n"));
    assert!(install::has_release(&dir.0.join("release")));
    install::verify_release(&dir.0.join("release"), &pubring, &dir.0, &tx).unwrap();

    dir.write("release/one.efi", "tampered\n");
    assert!(install::verify_release(&dir.0.join("release"), &pubring, &dir.0, &tx).is_err());

    dir.write("release/one.efi", "hello\n");
    dir.write("release/extra.efi", "hello\n");
    assert!(install::verify_release(&dir.0.join("release"), &pubring, &dir.0, &tx).is_err());
    assert!(!install::has_release(&dir.0.join("templates")));
}

#[test]
fn download_progress_is_the_last_percentage() {
    assert_eq!(
        serve::percent("Got 45% of https://example/usr.raw"),
        Some(0.45)
    );
    assert_eq!(serve::percent("10% then 100%"), Some(1.0));
    assert_eq!(serve::percent("no number here"), None);
    assert_eq!(serve::percent("Got % of it"), None);
    assert_eq!(serve::percent("250%"), None);
}

#[test]
fn requests_parse_and_name_a_device() {
    assert_eq!(
        serve::parse_request(r#"{"method":"disks"}"#),
        Ok(Request::Disks)
    );
    assert_eq!(
        serve::parse_request(r#"{"method":"install","disk":"/dev/vda"}"#),
        Ok(Request::Install("/dev/vda".into()))
    );
    assert_eq!(
        serve::parse_request(r#"{"method":"reboot"}"#),
        Ok(Request::Reboot)
    );
    assert_eq!(
        serve::parse_request(r#"{"method":"power_off"}"#),
        Ok(Request::PowerOff)
    );
    assert!(serve::parse_request(r#"{"method":"install","disk":"vda"}"#).is_err());
    assert!(serve::parse_request(r#"{"method":"install"}"#).is_err());
    assert!(serve::parse_request(r#"{"method":"format"}"#).is_err());
    assert!(serve::parse_request("not json").is_err());
}

#[test]
fn steps_and_disks_as_derisk_reads_them() {
    let steps = serve::steps_event();
    assert_eq!(steps["event"], "steps");
    assert_eq!(
        steps["labels"].as_array().unwrap().len(),
        install::Step::ALL.len()
    );
    let step = serve::step_event(install::Step::Download, Some(0.5));
    assert_eq!(step["index"], 2);
    assert_eq!(step["count"], 4);
    assert_eq!(step["fraction"], 0.5);
    assert!(serve::step_event(install::Step::Partition, None)["fraction"].is_null());

    let disk = Disk {
        path: "/dev/vda".into(),
        name: "vda".into(),
        model: "QEMU".into(),
        size: 21_474_836_480,
        removable: false,
    };
    let event = serve::disks_event(&[disk]);
    assert_eq!(event["event"], "disks");
    assert_eq!(event["disks"][0]["path"], "/dev/vda");
    assert_eq!(event["disks"][0]["size"], 21_474_836_480u64);
}

#[test]
fn an_install_is_refused_for_a_disk_not_offered() {
    let config = serve::Config {
        name: "LosOS Desktop".into(),
        repart_definitions: PathBuf::from("/nonexistent"),
        sysupdate_templates: PathBuf::from("/nonexistent"),
        esp: PathBuf::from("/nonexistent"),
        medium: "/nonexistent".into(),
        work: PathBuf::from("/nonexistent"),
        source: None,
        index: None,
        local_source: None,
        bios_boot: None,
        release_repo: None,
        release_version: None,
    };
    let input = "{\"method\":\"install\",\"disk\":\"/dev/not-a-disk\"}\nbogus\n";
    let (tx, rx) = std::sync::mpsc::channel();
    struct Sink(std::sync::mpsc::Sender<u8>);
    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            buf.iter().for_each(|b| {
                let _ = self.0.send(*b);
            });
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serve::run(&config, input.as_bytes(), Sink(tx));
    let out = String::from_utf8(rx.iter().collect()).unwrap();
    let events: Vec<serde_json::Value> = out
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(events[0]["event"], "hello");
    assert_eq!(events[0]["name"], "LosOS Desktop");
    assert_eq!(events[1]["event"], "failed");
    assert!(
        events[1]["message"]
            .as_str()
            .unwrap()
            .contains("/dev/not-a-disk")
    );
    assert_eq!(events[2]["event"], "failed");
    assert_eq!(events.len(), 3);
}

#[test]
fn hello_names_a_release_disk_only_when_it_holds_a_release() {
    let dir = Scratch::new("hello");
    let mut config = serve::Config {
        name: "LosOS Desktop".into(),
        repart_definitions: PathBuf::from("/nonexistent"),
        sysupdate_templates: PathBuf::from("/nonexistent"),
        esp: PathBuf::from("/nonexistent"),
        medium: "/nonexistent".into(),
        work: PathBuf::from("/nonexistent"),
        source: Some("https://proxy.example/".into()),
        index: None,
        local_source: Some(dir.0.join("release")),
        bios_boot: None,
        release_repo: None,
        release_version: None,
    };
    assert!(serve::hello(&config).get("release").is_none());
    dir.write("release/SHA256SUMS", "");
    assert_eq!(
        serve::hello(&config)["release"],
        dir.0.join("release").to_string_lossy().as_ref()
    );
    config.local_source = None;
    assert!(serve::hello(&config).get("release").is_none());
}

#[test]
fn sources_are_read_from_the_transfers() {
    let dir = Scratch::new("patterns");
    dir.write(
        "10-usr.transfer",
        "[Source]\nType=url-file\nPath=@URL@\nMatchPattern=os_@v_usr_@u.raw.xz\n\
         [Target]\nType=partition\nMatchPattern=os_@v\n",
    );
    dir.write(
        "30-uki.transfer",
        "[Transfer]\nProtectVersion=%A\n[Source]\nMatchPattern=os_@v_x86_64.efi\n\
         [Target]\nMatchPattern=os_@v+@l-@d.efi os_@v.efi\n",
    );
    dir.write("README", "MatchPattern=not_a_transfer\n");
    assert_eq!(
        stage::source_patterns(&dir.0).unwrap(),
        ["os_@v_usr_@u.raw.xz", "os_@v_x86_64.efi"]
    );
}

#[test]
fn a_pattern_gives_the_version_it_matched() {
    let v = |p, n| stage::version_of(p, n);
    assert_eq!(
        v(
            "os_@v_usr-x86-64_@u.raw.xz",
            "os_20261016.232901_usr-x86-64_0b8e4a52-6c5c-4a1e-9f4e-0d1c2b3a4f5e.raw.xz"
        )
        .as_deref(),
        Some("20261016.232901")
    );
    // The version has dots in it, and so does what follows it.
    assert_eq!(
        v("os_@v.grub", "os_20261016.232901.grub").as_deref(),
        Some("20261016.232901")
    );
    assert_eq!(
        v("os_@v_x86_64.efi", "os_20261016.232901_aarch64.efi"),
        None
    );
    assert_eq!(v("os_@v_x86_64.efi", "os__x86_64.efi"), None);
    assert_eq!(v("a@@b", "a@b").as_deref(), Some(""));
}

fn file(urls: &[&str], size: u64) -> github::File {
    github::File {
        urls: urls.iter().map(|u| (*u).to_owned()).collect(),
        size,
    }
}

#[test]
fn only_what_the_transfers_read_is_fetched() {
    let mut files = std::collections::BTreeMap::new();
    for name in [
        "SHA256SUMS",
        "SHA256SUMS.gpg",
        "os_1.0_usr_u1.raw.xz",
        "os_1.0_x86_64.efi",
        "os_1.0_x86_64-installer.iso",
        "os_1.1_usr_u2.raw.xz",
        "os_1.1_x86_64.efi",
        "os_1.2_usr_u3.raw.xz",
    ] {
        files.insert(
            name.to_owned(),
            file(&[&format!("https://g.example/{name}")], 1),
        );
    }
    let patterns = [
        "os_@v_usr_@u.raw.xz".to_owned(),
        "os_@v_x86_64.efi".to_owned(),
    ];

    let names = |picked: std::collections::BTreeMap<String, github::File>| {
        picked.into_keys().collect::<Vec<_>>()
    };
    // The release's own version, and nothing else of it: no ISO.
    assert_eq!(
        names(stage::wanted(&files, &patterns, Some("1.0")).unwrap()),
        [
            "SHA256SUMS",
            "SHA256SUMS.gpg",
            "os_1.0_usr_u1.raw.xz",
            "os_1.0_x86_64.efi"
        ]
    );
    // The channel's newest with every file: 1.2 has no UKI.
    assert_eq!(
        names(stage::wanted(&files, &patterns, None).unwrap()),
        [
            "SHA256SUMS",
            "SHA256SUMS.gpg",
            "os_1.1_usr_u2.raw.xz",
            "os_1.1_x86_64.efi"
        ]
    );
    assert!(stage::wanted(&files, &patterns, Some("1.2")).is_err());
    files.remove("SHA256SUMS.gpg");
    assert!(stage::wanted(&files, &patterns, Some("1.0")).is_err());
}

#[test]
fn the_channel_index_reads_as_files() {
    let files = stage::index_files(
        "SHA256SUMS 1096 https://g.example/t/SHA256SUMS-x86_64\n\
         os_usr.raw.xz 30 https://g.example/t/os_usr.raw.xz.part-00 https://g.example/t/os_usr.raw.xz.part-01\n\
         broken\n\
         nosize x https://g.example/t/nosize\n",
    );
    assert_eq!(files.len(), 2);
    assert_eq!(files["SHA256SUMS"].size, 1096);
    assert_eq!(
        files["os_usr.raw.xz"].urls,
        [
            "https://g.example/t/os_usr.raw.xz.part-00",
            "https://g.example/t/os_usr.raw.xz.part-01"
        ]
    );
}

#[test]
fn aria2_saves_every_part_under_its_own_name() {
    let mut files = std::collections::BTreeMap::new();
    files.insert(
        "os_usr.raw.xz".to_owned(),
        file(
            &[
                "https://g.example/t/os_usr.raw.xz.part-00",
                "https://g.example/t/os_usr.raw.xz.part-01?x=1",
            ],
            30,
        ),
    );
    files.insert(
        "SHA256SUMS".to_owned(),
        file(&["https://g.example/t/SHA256SUMS-x86_64"], 1),
    );
    assert_eq!(
        stage::aria2_input(&files),
        "https://g.example/t/SHA256SUMS-x86_64\n  out=SHA256SUMS-x86_64\n\
         https://g.example/t/os_usr.raw.xz.part-00\n  out=os_usr.raw.xz.part-00\n\
         https://g.example/t/os_usr.raw.xz.part-01?x=1\n  out=os_usr.raw.xz.part-01\n"
    );
    // A 2.5 GiB /usr gets its own size, ext4's share and some room on top.
    let mut usr = std::collections::BTreeMap::new();
    usr.insert(
        "u".to_owned(),
        file(&["https://g.example/u"], 2_621_440_000),
    );
    let size = stage::scratch_size(&usr);
    assert_eq!(size % (1 << 20), 0);
    assert!(size > 2_621_440_000 + (256 << 20));
    assert!(size < 4 << 30);
    assert!((1..=stage::MAX_CONNECTIONS).contains(&stage::connections()));
}

#[test]
fn the_scratch_partition_is_found_in_reparts_table() {
    let table = r#"[
        {"type":"esp","label":"ESP","node":"/dev/nvme0n1p1","activity":"unchanged"},
        {"type":"linux-generic","label":"losos-download","node":"/dev/nvme0n1p6","activity":"create"}
    ]"#;
    let node = stage::scratch_node(table).unwrap();
    assert_eq!(node, PathBuf::from("/dev/nvme0n1p6"));
    assert_eq!(stage::partition_number(&node), Some(6));
    assert_eq!(stage::partition_number("/dev/vda12".as_ref()), Some(12));
    assert_eq!(stage::partition_number("/dev/vda".as_ref()), None);
    assert!(stage::scratch_node(r#"[{"type":"esp","node":"/dev/vda1"}]"#).is_err());
}

#[test]
fn the_mirror_serves_a_split_file_whole() {
    use std::io::{Read, Write};

    let dir = Scratch::new("served");
    dir.write("u.part-00", "hello, ");
    dir.write("u.part-01", "world");
    let mut files = std::collections::BTreeMap::new();
    files.insert(
        "u.raw.xz".to_owned(),
        stage::Staged {
            paths: vec![dir.0.join("u.part-00"), dir.0.join("u.part-01")],
            size: 12,
        },
    );
    let mirror = github::Mirror::new(files).unwrap();
    let addr = mirror.url().unwrap();
    let addr = addr
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_owned();
    let (events, _log) = std::sync::mpsc::channel();
    let get = |path: &str| {
        let mut stream = std::net::TcpStream::connect(&addr).unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer
    };
    let (whole, missing) = mirror.serve(&events, || (get("/u.raw.xz"), get("/other")));
    assert!(whole.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(whole.contains("Content-Length: 12\r\n"));
    assert!(whole.ends_with("\r\n\r\nhello, world"), "{whole:?}");
    assert!(missing.starts_with("HTTP/1.1 404"));
}
