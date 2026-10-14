// The chrome.* (and browser.*) API an extension's scripts see, run before
// any of them in every context Danube gives an extension: each content
// script's isolated world, the background page, the popup, an offscreen
// document, an options or other extension page in a tab, and an
// extension's iframe in a web page. WPE WebKit has no extension runtime,
// so this one is Danube's: every call becomes one message to the engine
// (runtime.rs) through WebKit's script message handler, whose reply
// settles the promise or calls the callback. What the engine has to tell
// a context (a message, a port, an event) comes back as the answer to a
// `wait` the context always keeps open; WebKit gives the engine no other
// way into a frame other than the main one.
//
// `__DANUBE_CONFIG__` is replaced by the engine with this context's
// configuration before injection: the extension's id and manifest, its
// messages for chrome.i18n, the world and kind the script runs in, and the
// secret that lets a main-world context prove it is an extension page.
(function () {
  "use strict";
  if (globalThis.__danube) return;
  const config = __DANUBE_CONFIG__;
  const EXT = config.id;
  const BASE = "chrome-extension://" + EXT + "/";
  const handler = globalThis.webkit && globalThis.webkit.messageHandlers && globalThis.webkit.messageHandlers.danube;
  if (!handler) return;
  // The handler is taken off `window.webkit` so that a page script cannot
  // find and call it; a closure keeps it for this file.
  const post = (text) => handler.postMessage(text);
  try {
    delete globalThis.webkit.messageHandlers.danube;
  } catch (e) {}

  // Every context tells the engine who it is first; calls made before the
  // answer wait for it, so a content script may message at once.
  let ctx = null;
  let me = { tab: null, frameId: 0, kind: config.kind, url: location.href };
  let leaving = false;
  function hello() {
    return raw(null, "hello", [{
      secret: config.secret,
      ext: EXT,
      url: location.href,
      world: config.world,
      kind: config.kind,
      top: globalThis === globalThis.top,
    }]).then((answer) => {
      ctx = answer.ctx;
      me = Object.assign(me, answer);
      return answer;
    });
  }
  let ready = hello();
  const first = ready;
  ready.catch((e) => console.warn("danube: extension context not registered:", e));

  async function raw(c, m, a) {
    const text = await post(JSON.stringify({ c, m, a }));
    return text === undefined || text === null || text === "" ? undefined : JSON.parse(text);
  }
  // The engine drops a context that has not come for its deliveries in a
  // while, taking its page for gone; a page that was only busy (deriving
  // a key, say) registers again and carries on.
  async function rejoin(e) {
    if (leaving || !String((e && e.message) || e).includes("unknown context")) throw e;
    ready = hello();
    await ready;
  }
  async function call(m, ...a) {
    await ready;
    try {
      return await raw(ctx, m, a);
    } catch (e) {
      await rejoin(e);
      return raw(ctx, m, a);
    }
  }
  // Fire-and-forget: the engine's answer is not waited for.
  function tell(m, ...a) {
    call(m, ...a).catch(() => {});
  }
  globalThis.addEventListener("pagehide", () => {
    leaving = true;
    tell("bye");
  });

  // --- Events and the callback/promise convention ------------------------

  function Event() {
    const listeners = [];
    const event = {
      addListener(fn) {
        if (typeof fn === "function" && !listeners.includes(fn)) listeners.push(fn);
      },
      removeListener(fn) {
        const i = listeners.indexOf(fn);
        if (i >= 0) listeners.splice(i, 1);
      },
      hasListener(fn) {
        return listeners.includes(fn);
      },
      hasListeners() {
        return listeners.length > 0;
      },
      // Calls every listener, returning what each gave.
      _fire(...args) {
        return listeners.slice().map((fn) => {
          try {
            return fn(...args);
          } catch (e) {
            console.error("danube: an extension listener failed:", e);
            return undefined;
          }
        });
      },
    };
    return event;
  }

  let lastError = null;
  // Chrome's convention: with a callback last, the result goes to it and a
  // failure is chrome.runtime.lastError while it runs; without one, a
  // promise. `fn` returns a promise either way.
  function api(fn) {
    return function (...args) {
      let callback = null;
      if (args.length && typeof args[args.length - 1] === "function") callback = args.pop();
      const promise = Promise.resolve().then(() => fn(...args));
      if (!callback) return promise;
      promise.then(
        (value) => callback(value),
        (error) => {
          lastError = { message: String((error && error.message) || error) };
          try {
            callback();
          } finally {
            lastError = null;
          }
        }
      );
      return undefined;
    };
  }
  // A call the engine answers, with the arguments as given.
  const remote = (m) => api((...a) => call(m, ...a));
  const noop = (value) => api(async () => value);

  // --- Messaging ------------------------------------------------------

  const runtime = {};
  runtime.id = EXT;
  runtime.getURL = (path) => BASE + String(path || "").replace(/^\/+/, "");
  runtime.getManifest = () => JSON.parse(JSON.stringify(config.manifest));
  runtime.onMessage = Event();
  runtime.onMessageExternal = Event();
  runtime.onConnect = Event();
  runtime.onConnectExternal = Event();
  runtime.onInstalled = Event();
  runtime.onStartup = Event();
  runtime.onSuspend = Event();
  runtime.onSuspendCanceled = Event();
  runtime.onUpdateAvailable = Event();
  runtime.onRestartRequired = Event();
  Object.defineProperty(runtime, "lastError", { get: () => lastError, enumerable: true });

  // runtime.sendMessage(message), (message, options), (extensionId,
  // message), (extensionId, message, options), each with an optional
  // callback, which `api` already took.
  runtime.sendMessage = api((...args) => {
    // With two or more arguments and a string (or nothing) first, that is
    // an extension id and the message comes second.
    const [a, b] = args;
    const message = args.length >= 2 && (typeof a === "string" || a === null || a === undefined) ? b : a;
    return call("runtime.sendMessage", message === undefined ? null : message);
  });
  runtime.getPlatformInfo = noop(config.platform);
  runtime.getBrowserInfo = noop({ name: "Danube", vendor: "LosOS", version: config.version, buildID: "" });
  runtime.getContexts = remote("runtime.getContexts");
  runtime.openOptionsPage = remote("runtime.openOptionsPage");
  runtime.reload = () => tell("runtime.reload");
  runtime.requestUpdateCheck = noop(["no_update", { version: "" }]);
  runtime.setUninstallURL = noop(undefined);
  runtime.getBackgroundPage = noop(null);
  runtime.restart = () => {};
  runtime.connectNative = () => {
    throw new Error("Native messaging is not available in Danube");
  };
  runtime.sendNativeMessage = api(async () => {
    throw new Error("Native messaging is not available in Danube");
  });
  runtime.OnInstalledReason = { INSTALL: "install", UPDATE: "update", CHROME_UPDATE: "chrome_update", SHARED_MODULE_UPDATE: "shared_module_update" };
  runtime.PlatformOs = { MAC: "mac", WIN: "win", ANDROID: "android", CROS: "cros", LINUX: "linux", OPENBSD: "openbsd" };
  runtime.PlatformArch = { ARM: "arm", ARM64: "arm64", X86_32: "x86-32", X86_64: "x86-64" };
  runtime.ContextType = { TAB: "TAB", POPUP: "POPUP", BACKGROUND: "BACKGROUND", OFFSCREEN_DOCUMENT: "OFFSCREEN_DOCUMENT", SIDE_PANEL: "SIDE_PANEL" };
  runtime.RequestUpdateCheckStatus = { THROTTLED: "throttled", NO_UPDATE: "no_update", UPDATE_AVAILABLE: "update_available" };
  runtime.MessageSender = function () {};
  runtime.Port = function () {};

  // Ports. One made here has its engine id once `runtime.connect` is
  // answered, and queues what is posted until then; one the engine
  // delivers knows its id from the start.
  const ports = new Map();
  function makePort(name, sender, idPromise) {
    let id = null;
    let open = true;
    const port = {
      name,
      sender,
      onMessage: Event(),
      onDisconnect: Event(),
      postMessage(message) {
        if (!open) throw new Error("Attempting to use a disconnected port object");
        idPromise.then((i) => call("port.post", i, message === undefined ? null : message)).catch(() => {});
      },
      disconnect() {
        if (!open) return;
        open = false;
        idPromise.then((i) => {
          ports.delete(i);
          tell("port.disconnect", i);
        });
      },
      _closed() {
        if (!open) return;
        open = false;
        if (id !== null) ports.delete(id);
        port.onDisconnect._fire(port);
      },
    };
    idPromise.then((i) => {
      id = i;
      if (open) ports.set(i, port);
      else ports.delete(i);
    });
    return port;
  }
  runtime.connect = (...args) => {
    let info = args.length >= 2 ? args[1] : typeof args[0] === "string" ? undefined : args[0];
    const name = (info && info.name) || "";
    return makePort(name, undefined, call("runtime.connect", name, null, null).then((r) => r.port));
  };

  // --- Storage --------------------------------------------------------

  const storage = { onChanged: Event() };
  for (const area of ["local", "sync", "session", "managed"]) {
    storage[area] = {
      get: api((keys) => call("storage.get", area, keys === undefined ? null : keys)),
      set: api((items) => call("storage.set", area, items)),
      remove: api((keys) => call("storage.remove", area, keys)),
      clear: api(() => call("storage.clear", area)),
      getBytesInUse: api((keys) => call("storage.getBytesInUse", area, keys === undefined ? null : keys)),
      setAccessLevel: noop(undefined),
      getKeys: api(async () => Object.keys(await call("storage.get", area, null))),
      onChanged: Event(),
      QUOTA_BYTES: area === "sync" ? 102400 : 10485760,
    };
  }
  storage.sync.QUOTA_BYTES_PER_ITEM = 8192;
  storage.sync.MAX_ITEMS = 512;
  storage.sync.MAX_WRITE_OPERATIONS_PER_HOUR = 1800;
  storage.sync.MAX_WRITE_OPERATIONS_PER_MINUTE = 120;
  storage.AccessLevel = { TRUSTED_CONTEXTS: "TRUSTED_CONTEXTS", TRUSTED_AND_UNTRUSTED_CONTEXTS: "TRUSTED_AND_UNTRUSTED_CONTEXTS" };

  // --- Tabs and windows ------------------------------------------------

  const tabs = {
    TAB_ID_NONE: -1,
    onCreated: Event(),
    onUpdated: Event(),
    onActivated: Event(),
    onRemoved: Event(),
    onReplaced: Event(),
    onMoved: Event(),
    onAttached: Event(),
    onDetached: Event(),
    onHighlighted: Event(),
    onZoomChange: Event(),
    query: remote("tabs.query"),
    get: remote("tabs.get"),
    getCurrent: api(() => call("tabs.getCurrent")),
    create: remote("tabs.create"),
    update: api((...a) => (a.length === 1 ? call("tabs.update", null, a[0]) : call("tabs.update", a[0], a[1]))),
    remove: remote("tabs.remove"),
    reload: remote("tabs.reload"),
    goBack: remote("tabs.goBack"),
    goForward: remote("tabs.goForward"),
    highlight: noop({ id: 1 }),
    move: noop(undefined),
    getZoom: noop(1),
    setZoom: noop(undefined),
    detectLanguage: noop(config.lang),
    captureVisibleTab: api(async () => {
      throw new Error("tabs.captureVisibleTab is not available in Danube");
    }),
    sendMessage: api((tabId, message, options) => call("tabs.sendMessage", tabId, message === undefined ? null : message, options || null)),
    connect: (tabId, info) => {
      const name = (info && info.name) || "";
      const frameId = info && info.frameId !== undefined ? info.frameId : null;
      return makePort(name, undefined, call("runtime.connect", name, tabId, frameId).then((r) => r.port));
    },
    // Manifest V2's way to run code in a tab; scripting.* is V3's.
    executeScript: api((...a) => {
      const [tabId, details] = a.length === 1 ? [null, a[0]] : a;
      return call("scripting.executeScript", {
        target: { tabId, allFrames: !!details.allFrames, frameIds: details.frameId !== undefined ? [details.frameId] : undefined },
        files: details.file ? [details.file] : undefined,
        code: details.code,
        runAt: details.runAt,
      });
    }),
    insertCSS: api((...a) => {
      const [tabId, details] = a.length === 1 ? [null, a[0]] : a;
      return call("scripting.insertCSS", {
        target: { tabId, allFrames: !!details.allFrames, frameIds: details.frameId !== undefined ? [details.frameId] : undefined },
        files: details.file ? [details.file] : undefined,
        css: details.code,
      });
    }),
    removeCSS: noop(undefined),
    Tab: function () {},
    TabStatus: { UNLOADED: "unloaded", LOADING: "loading", COMPLETE: "complete" },
    WindowType: { NORMAL: "normal", POPUP: "popup", PANEL: "panel", APP: "app", DEVTOOLS: "devtools" },
  };

  const windows = {
    WINDOW_ID_NONE: -1,
    WINDOW_ID_CURRENT: -2,
    onCreated: Event(),
    onRemoved: Event(),
    onFocusChanged: Event(),
    onBoundsChanged: Event(),
    get: remote("windows.get"),
    getCurrent: remote("windows.getCurrent"),
    getLastFocused: remote("windows.getCurrent"),
    getAll: remote("windows.getAll"),
    create: remote("windows.create"),
    update: remote("windows.update"),
    remove: remote("windows.remove"),
    WindowType: { NORMAL: "normal", POPUP: "popup", PANEL: "panel", APP: "app", DEVTOOLS: "devtools" },
    WindowState: { NORMAL: "normal", MINIMIZED: "minimized", MAXIMIZED: "maximized", FULLSCREEN: "fullscreen", LOCKED_FULLSCREEN: "locked-fullscreen" },
    CreateType: { NORMAL: "normal", POPUP: "popup", PANEL: "panel" },
  };

  // --- Scripting ----------------------------------------------------

  const scripting = {
    ExecutionWorld: { ISOLATED: "ISOLATED", MAIN: "MAIN" },
    executeScript: api((injection) => {
      const copy = Object.assign({}, injection);
      if (typeof copy.func === "function") {
        // The function travels as source and runs in the target frame with
        // its arguments as JSON, as Chrome does it.
        copy.func = String(copy.func);
        copy.args = (copy.args || []).map((a) => (a === undefined ? null : a));
      } else if (copy.function) {
        copy.func = String(copy.function);
        delete copy.function;
      }
      return call("scripting.executeScript", copy);
    }),
    insertCSS: remote("scripting.insertCSS"),
    removeCSS: remote("scripting.removeCSS"),
    registerContentScripts: remote("scripting.registerContentScripts"),
    unregisterContentScripts: remote("scripting.unregisterContentScripts"),
    updateContentScripts: remote("scripting.updateContentScripts"),
    getRegisteredContentScripts: remote("scripting.getRegisteredContentScripts"),
  };

  // --- Smaller namespaces -----------------------------------------------

  const alarms = {
    onAlarm: Event(),
    create: api((...a) => (a.length === 1 ? call("alarms.create", "", a[0]) : call("alarms.create", a[0], a[1]))),
    get: api((name) => call("alarms.get", name === undefined ? "" : name)),
    getAll: remote("alarms.getAll"),
    clear: api((name) => call("alarms.clear", name === undefined ? "" : name)),
    clearAll: remote("alarms.clearAll"),
  };

  const i18n = {
    getUILanguage: () => config.lang,
    getAcceptLanguages: noop([config.lang]),
    detectLanguage: noop({ isReliable: false, languages: [] }),
    getMessage(name, substitutions) {
      const entry = config.messages[String(name).toLowerCase()];
      if (!entry) return "";
      const subs = substitutions === undefined ? [] : Array.isArray(substitutions) ? substitutions : [substitutions];
      const sub = (text) => String(text).replace(/\$(\d)/g, (_, n) => (n === "0" ? "" : subs[n - 1] === undefined ? "" : String(subs[n - 1])));
      let out = String(entry.message || "");
      out = out.replace(/\$([A-Za-z0-9_@]+)\$/g, (whole, key) => {
        const placeholders = entry.placeholders || {};
        const found = Object.keys(placeholders).find((k) => k.toLowerCase() === key.toLowerCase());
        return found ? sub(placeholders[found].content || "") : whole;
      });
      return sub(out).replace(/\$\$/g, "$");
    },
  };

  const action = {
    onClicked: Event(),
    onUserSettingsChanged: Event(),
    setBadgeText: remote("action.setBadgeText"),
    getBadgeText: remote("action.getBadgeText"),
    setBadgeBackgroundColor: noop(undefined),
    getBadgeBackgroundColor: noop([0, 0, 0, 0]),
    setBadgeTextColor: noop(undefined),
    getBadgeTextColor: noop([255, 255, 255, 255]),
    setTitle: remote("action.setTitle"),
    getTitle: remote("action.getTitle"),
    setIcon: noop(undefined),
    setPopup: remote("action.setPopup"),
    getPopup: remote("action.getPopup"),
    enable: noop(undefined),
    disable: noop(undefined),
    isEnabled: noop(true),
    getUserSettings: noop({ isOnToolbar: true }),
    openPopup: remote("action.openPopup"),
  };

  const commands = {
    onCommand: Event(),
    getAll: remote("commands.getAll"),
  };

  const notifications = {
    onClicked: Event(),
    onClosed: Event(),
    onButtonClicked: Event(),
    onShowSettings: Event(),
    onPermissionLevelChanged: Event(),
    create: api((...a) => (a.length === 1 ? call("notifications.create", "", a[0]) : call("notifications.create", a[0] || "", a[1]))),
    update: noop(false),
    clear: noop(true),
    getAll: noop({}),
    getPermissionLevel: noop("granted"),
    TemplateType: { BASIC: "basic", IMAGE: "image", LIST: "list", PROGRESS: "progress" },
    PermissionLevel: { GRANTED: "granted", DENIED: "denied" },
  };

  const downloads = {
    onCreated: Event(),
    onChanged: Event(),
    onErased: Event(),
    onDeterminingFilename: Event(),
    download: remote("downloads.download"),
    search: noop([]),
    cancel: noop(undefined),
    pause: noop(undefined),
    resume: noop(undefined),
    erase: noop([]),
    removeFile: noop(undefined),
    show: noop(undefined),
    showDefaultFolder: () => {},
    open: () => {},
    setShelfEnabled: () => {},
    State: { IN_PROGRESS: "in_progress", INTERRUPTED: "interrupted", COMPLETE: "complete" },
    InterruptReason: {},
  };

  const webNavigation = {
    onBeforeNavigate: Event(),
    onCommitted: Event(),
    onDOMContentLoaded: Event(),
    onCompleted: Event(),
    onErrorOccurred: Event(),
    onCreatedNavigationTarget: Event(),
    onReferenceFragmentUpdated: Event(),
    onTabReplaced: Event(),
    onHistoryStateUpdated: Event(),
    getFrame: remote("webNavigation.getFrame"),
    getAllFrames: remote("webNavigation.getAllFrames"),
  };

  const permissions = {
    onAdded: Event(),
    onRemoved: Event(),
    contains: remote("permissions.contains"),
    request: remote("permissions.request"),
    remove: noop(true),
    getAll: remote("permissions.getAll"),
  };

  const offscreen = {
    Reason: {
      TESTING: "TESTING", AUDIO_PLAYBACK: "AUDIO_PLAYBACK", IFRAME_SCRIPTING: "IFRAME_SCRIPTING", DOM_SCRAPING: "DOM_SCRAPING",
      BLOBS: "BLOBS", DOM_PARSER: "DOM_PARSER", USER_MEDIA: "USER_MEDIA", DISPLAY_MEDIA: "DISPLAY_MEDIA", WEB_RTC: "WEB_RTC",
      CLIPBOARD: "CLIPBOARD", LOCAL_STORAGE: "LOCAL_STORAGE", WORKERS: "WORKERS", BATTERY_STATUS: "BATTERY_STATUS",
      MATCH_MEDIA: "MATCH_MEDIA", GEOLOCATION: "GEOLOCATION",
    },
    createDocument: remote("offscreen.createDocument"),
    closeDocument: remote("offscreen.closeDocument"),
    hasDocument: remote("offscreen.hasDocument"),
  };

  // A privacy setting the extension may read (and think it set): Danube
  // has no password saving or form filling of its own to turn off.
  function setting(value) {
    return {
      get: noop({ value, levelOfControl: "not_controllable" }),
      set: noop(undefined),
      clear: noop(undefined),
      onChange: Event(),
    };
  }
  const privacy = {
    services: {
      passwordSavingEnabled: setting(false),
      autofillEnabled: setting(false),
      autofillAddressEnabled: setting(false),
      autofillCreditCardEnabled: setting(false),
      safeBrowsingEnabled: setting(false),
      searchSuggestEnabled: setting(false),
      spellingServiceEnabled: setting(false),
      translationServiceEnabled: setting(false),
      alternateErrorPagesEnabled: setting(false),
    },
    network: { networkPredictionEnabled: setting(false), webRTCIPHandlingPolicy: setting("default") },
    websites: { thirdPartyCookiesAllowed: setting(false), hyperlinkAuditingEnabled: setting(false), referrersEnabled: setting(true), doNotTrackEnabled: setting(false) },
  };

  const management = {
    getSelf: api(async () => ({
      id: EXT,
      name: config.manifest.name,
      shortName: config.manifest.short_name || config.manifest.name,
      description: config.manifest.description || "",
      version: config.manifest.version,
      enabled: true,
      mayDisable: true,
      isApp: false,
      type: "extension",
      installType: "development",
      permissions: config.manifest.permissions || [],
      hostPermissions: config.manifest.host_permissions || [],
      homepageUrl: config.manifest.homepage_url,
      optionsUrl: config.manifest.options_page ? runtime.getURL(config.manifest.options_page) : "",
    })),
    getAll: noop([]),
    get: noop(undefined),
    onEnabled: Event(),
    onDisabled: Event(),
    onInstalled: Event(),
    onUninstalled: Event(),
  };

  const idle = {
    onStateChanged: Event(),
    queryState: noop("active"),
    setDetectionInterval: () => {},
    getAutoLockDelay: noop(0),
  };

  let nextMenu = 1;
  const contextMenus = {
    ACTION_MENU_TOP_LEVEL_LIMIT: 6,
    onClicked: Event(),
    create: (props, callback) => {
      const id = props && props.id !== undefined ? props.id : nextMenu++;
      if (callback) Promise.resolve().then(callback);
      return id;
    },
    update: noop(undefined),
    remove: noop(undefined),
    removeAll: noop(undefined),
    ContextType: { ALL: "all", PAGE: "page", FRAME: "frame", SELECTION: "selection", LINK: "link", EDITABLE: "editable", IMAGE: "image", VIDEO: "video", AUDIO: "audio", LAUNCHER: "launcher", BROWSER_ACTION: "browser_action", PAGE_ACTION: "page_action", ACTION: "action" },
    ItemType: { NORMAL: "normal", CHECKBOX: "checkbox", RADIO: "radio", SEPARATOR: "separator" },
  };

  // Request interception is WebKit's content blockers' job (docs/adblock.md),
  // not an extension's; listeners are accepted and never called.
  function webRequestEvent() {
    const event = Event();
    event.addListener = () => {};
    return event;
  }
  const webRequest = {
    MAX_HANDLER_BEHAVIOR_CHANGED_CALLS_PER_10_MINUTES: 20,
    onBeforeRequest: webRequestEvent(),
    onBeforeSendHeaders: webRequestEvent(),
    onSendHeaders: webRequestEvent(),
    onHeadersReceived: webRequestEvent(),
    onAuthRequired: webRequestEvent(),
    onResponseStarted: webRequestEvent(),
    onBeforeRedirect: webRequestEvent(),
    onCompleted: webRequestEvent(),
    onErrorOccurred: webRequestEvent(),
    handlerBehaviorChanged: noop(undefined),
  };

  const cookies = {
    onChanged: Event(),
    get: noop(null),
    getAll: noop([]),
    set: noop(null),
    remove: noop(null),
    getAllCookieStores: noop([{ id: "0", tabIds: [] }]),
  };

  const sidePanel = {
    setOptions: noop(undefined),
    getOptions: noop({ enabled: false }),
    setPanelBehavior: noop(undefined),
    getPanelBehavior: noop({ openPanelOnActionClick: false }),
    open: api(async () => {
      throw new Error("The side panel is not available in Danube");
    }),
  };

  const extension = {
    inIncognitoContext: false,
    getURL: runtime.getURL,
    // Other views run in their own web processes here, so none can be
    // handed over as a window; an extension that asks sees no views open.
    getViews: () => [],
    getBackgroundPage: () => null,
    isAllowedIncognitoAccess: noop(false),
    isAllowedFileSchemeAccess: noop(false),
    setUpdateUrlData: () => {},
    ViewType: { TAB: "tab", POPUP: "popup" },
    onRequest: Event(),
    onRequestExternal: Event(),
  };
  Object.defineProperty(extension, "lastError", { get: () => lastError, enumerable: true });

  const dom = {
    // A closed shadow root is WebKit's to keep; the open ones are enough
    // for form fields inside components.
    openOrClosedShadowRoot: (element) => (element && element.shadowRoot) || null,
  };

  const tabGroups = { TAB_GROUP_ID_NONE: -1, onCreated: Event(), onUpdated: Event(), onRemoved: Event(), onMoved: Event(), get: noop(undefined), query: noop([]), update: noop(undefined), move: noop(undefined) };
  const declarativeNetRequest = { updateDynamicRules: noop(undefined), getDynamicRules: noop([]), updateSessionRules: noop(undefined), getSessionRules: noop([]), updateEnabledRulesets: noop(undefined), getEnabledRulesets: noop([]), onRuleMatchedDebug: Event() };
  const history = { onVisited: Event(), onVisitRemoved: Event(), search: noop([]), addUrl: noop(undefined), deleteUrl: noop(undefined), deleteAll: noop(undefined) };
  const bookmarks = { onCreated: Event(), onRemoved: Event(), onChanged: Event(), getTree: noop([]), search: noop([]), create: noop(undefined), remove: noop(undefined) };
  const browsingData = { remove: noop(undefined), removeCache: noop(undefined), removeCookies: noop(undefined), removeLocalStorage: noop(undefined), settings: noop({ options: {}, dataToRemove: {}, dataRemovalPermitted: {} }) };
  const clipboard = { setImageData: noop(undefined), onClipboardDataChanged: Event() };
  const system = { display: { getInfo: noop([]) }, cpu: { getInfo: noop({ numOfProcessors: 1, archName: config.platform.arch, modelName: "", features: [], processors: [] }) }, memory: { getInfo: noop({ capacity: 0, availableCapacity: 0 }) } };
  const fontSettings = { getFont: noop({ fontId: "" }), getDefaultFontSize: noop({ pixelSize: 16 }) };
  const topSites = { get: noop([]) };
  const search = { query: noop(undefined), Disposition: { CURRENT_TAB: "CURRENT_TAB", NEW_TAB: "NEW_TAB", NEW_WINDOW: "NEW_WINDOW" } };
  const sessions = { MAX_SESSION_RESULTS: 25, getRecentlyClosed: noop([]), getDevices: noop([]), restore: noop(undefined), onChanged: Event() };
  const identity = { getAuthToken: api(async () => { throw new Error("identity is not available in Danube"); }), launchWebAuthFlow: api(async () => { throw new Error("identity is not available in Danube"); }), getRedirectURL: (path) => "https://" + EXT + ".chromiumapp.org/" + (path || ""), getProfileUserInfo: noop({ email: "", id: "" }), onSignInChanged: Event() };
  const omnibox = { onInputStarted: Event(), onInputChanged: Event(), onInputEntered: Event(), onInputCancelled: Event(), onDeleteSuggestion: Event(), setDefaultSuggestion: noop(undefined) };
  const power = { requestKeepAwake: () => {}, releaseKeepAwake: () => {}, reportActivity: noop(undefined) };
  const tts = { speak: noop(undefined), stop: () => {}, isSpeaking: noop(false), getVoices: noop([]) };
  const pageAction = action;
  const browserAction = action;
  const userScripts = { register: noop(undefined), getScripts: noop([]), unregister: noop(undefined), update: noop(undefined), configureWorld: noop(undefined) };

  const chrome = {
    runtime, storage, tabs, windows, scripting, alarms, i18n, action, browserAction, pageAction, commands, notifications,
    downloads, webNavigation, permissions, offscreen, privacy, management, idle, contextMenus, webRequest, cookies, sidePanel,
    extension, dom, tabGroups, declarativeNetRequest, history, bookmarks, browsingData, clipboard, system, fontSettings, topSites,
    search, sessions, identity, omnibox, power, tts, userScripts,
  };
  // The same object answers as `browser`, so the promise-based code written
  // for Firefox (or against webextension-polyfill) works unchanged.
  Object.defineProperty(globalThis, "chrome", { value: chrome, configurable: true, writable: true, enumerable: true });
  Object.defineProperty(globalThis, "browser", { value: chrome, configurable: true, writable: true, enumerable: true });

  // Extensions sniff the User-Agent for their browser; Chrome's paths are
  // the ones they test and ours implement, so this world sees Chrome's
  // string. The page's own world keeps WebKit's.
  if (config.ua) {
    try {
      Object.defineProperty(Object.getPrototypeOf(navigator), "userAgent", { get: () => config.ua, configurable: true });
      Object.defineProperty(Object.getPrototypeOf(navigator), "vendor", { get: () => "Google Inc.", configurable: true });
    } catch (e) {}
  }

  // Chrome has the explicit resource management symbols and WPE's
  // JavaScriptCore does not yet; code compiled by TypeScript for `using`
  // throws without them (Bitwarden's popup does). Its own documentation
  // gives these two lines as the polyfill.
  if (!Symbol.dispose) Object.defineProperty(Symbol, "dispose", { value: Symbol("Symbol.dispose") });
  if (!Symbol.asyncDispose) Object.defineProperty(Symbol, "asyncDispose", { value: Symbol("Symbol.asyncDispose") });

  // --- What the engine delivers ---------------------------------------

  const pendingResponses = new Set();
  function sender(s) {
    return s || { id: EXT };
  }
  function deliver(item) {
    switch (item.t) {
      case "message": {
        let answered = false;
        let async = false;
        const sendResponse = (response) => {
          if (answered) return;
          answered = true;
          tell("respond", item.id, response === undefined ? null : response);
        };
        const results = runtime.onMessage._fire(item.msg, sender(item.sender), sendResponse);
        for (const r of results) {
          if (r === true) async = true;
          else if (r && typeof r.then === "function") {
            // Firefox's convention: a promise is the response.
            async = true;
            r.then(sendResponse, (e) => sendResponse(undefined));
          }
        }
        if (!async && !answered) return { handled: false };
        return { handled: true };
      }
      case "connect": {
        if (!runtime.onConnect.hasListeners()) return { accepted: false };
        const port = makePort(item.name, sender(item.sender), Promise.resolve(item.port));
        ports.set(item.port, port);
        runtime.onConnect._fire(port);
        return { accepted: true };
      }
      case "portmsg": {
        const port = ports.get(item.port);
        if (port) port.onMessage._fire(item.msg, port);
        return { delivered: !!port };
      }
      case "disconnect": {
        const port = ports.get(item.port);
        if (port) port._closed();
        return {};
      }
      case "event": {
        const ns = chrome[item.ns];
        const event = ns && ns[item.ev];
        if (event && event._fire) event._fire(...item.args);
        // storage.onChanged also fires on the area itself.
        if (item.ns === "storage" && item.ev === "onChanged") {
          const area = storage[item.args[1]];
          if (area) area.onChanged._fire(item.args[0]);
        }
        return {};
      }
      case "exec": {
        // chrome.scripting.executeScript in this world; a promise it gives
        // is waited for, as Chrome does.
        const value = (0, eval)(item.code);
        return Promise.resolve(value).then((v) => ({ value: v === undefined ? null : v }));
      }
      case "command": {
        if (item.name === "_execute_action" || item.name === "_execute_browser_action") action.onClicked._fire(item.tab);
        else commands.onCommand._fire(item.name, item.tab);
        return {};
      }
      default:
        return {};
    }
  }

  // --- A service worker's page -------------------------------------------
  // A Manifest V3 background is a service worker; Danube runs it in a page
  // made for it, with the worker globals it reaches for.
  function serviceWorker() {
    const installed = [];
    const registration = {
      scope: BASE,
      active: { state: "activated", scriptURL: location.href, postMessage() {} },
      installing: null,
      waiting: null,
      navigationPreload: { enable: async () => {}, disable: async () => {}, getState: async () => ({ enabled: false }) },
      update: async () => registration,
      unregister: async () => true,
      addEventListener() {},
      removeEventListener() {},
    };
    globalThis.registration = registration;
    globalThis.clients = {
      matchAll: async () => [],
      get: async () => undefined,
      openWindow: async (url) => {
        await tabs.create({ url });
        return null;
      },
      claim: async () => {},
    };
    globalThis.skipWaiting = async () => {};
    globalThis.serviceWorker = { state: "activated", scriptURL: location.href, addEventListener() {}, removeEventListener() {} };
    globalThis.ServiceWorkerGlobalScope = Object.getPrototypeOf(globalThis).constructor;
    // A worker's importScripts: fetched synchronously, as a worker would,
    // and run in the global scope. The generated page is served with
    // 'unsafe-eval' for this alone.
    globalThis.importScripts = function (...urls) {
      for (const url of urls) {
        const request = new XMLHttpRequest();
        request.open("GET", new URL(url, location.href).href, false);
        request.send();
        if (request.status !== 200 && request.status !== 0) throw new Error("importScripts: cannot load " + url);
        (0, eval)(request.responseText + "\n//# sourceURL=" + url);
      }
    };
    return function started() {
      for (const type of ["install", "activate"]) {
        const event = new window.Event(type);
        event.waitUntil = (p) => Promise.resolve(p).catch(() => {});
        globalThis.dispatchEvent(event);
        if (typeof globalThis["on" + type] === "function") globalThis["on" + type](event);
      }
      tell("ready");
    };
  }

  // The engine's deliveries: one `wait` at a time, answered with whatever
  // has queued up; each item that wants an answer gets an `ack`.
  async function pump() {
    try {
      await ready;
    } catch (e) {
      return;
    }
    for (;;) {
      let items;
      try {
        items = await raw(ctx, "wait", []);
      } catch (e) {
        // The engine forgot this context: its page is going, and nothing
        // more will come, or it was busy and registers again.
        try {
          await rejoin(e);
        } catch (e) {
          return;
        }
        continue;
      }
      for (const item of items || []) {
        let result;
        try {
          result = deliver(item);
        } catch (e) {
          console.error("danube: an extension delivery failed:", e);
          result = { error: String(e) };
        }
        if (!item.ack) continue;
        Promise.resolve(result)
          .then(
            (value) => (value === undefined ? null : value),
            (e) => ({ error: String((e && e.message) || e) })
          )
          .then((value) => {
            // A result that cannot be sent (a DOM node, say) goes as null,
            // as Chrome's structured clone would refuse it too.
            try {
              JSON.stringify(value);
            } catch (e) {
              value = { value: null };
            }
            return raw(ctx, "ack", [item.seq, value]);
          })
          .catch(() => {});
      }
    }
  }
  pump();

  const danube = { call, ready: first.then(() => undefined) };
  if (config.kind === "background" && config.serviceWorker) {
    danube.swStarted = serviceWorker();
  } else if (config.kind === "background") {
    // A background page is ready once its scripts ran.
    addEventListener("load", () => tell("ready"));
  }
  Object.defineProperty(globalThis, "__danube", { value: danube, configurable: false, writable: false, enumerable: false });
})();
