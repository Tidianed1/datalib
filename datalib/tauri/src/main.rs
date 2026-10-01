//! Datalib Tauri shell.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod launcher;
mod raw_store;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use tauri::webview::{NewWindowFeatures, NewWindowResponse};
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder, Wry};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;

/// The spawned `datalib-http` child, managed in tauri state so the
/// exit handler can kill it. `None` until boot succeeds.
struct HttpChild(Mutex<Option<Child>>);

/// The data root the backend was started on; `None` until boot succeeds.
struct DataRoot(Mutex<Option<PathBuf>>);

#[tauri::command]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// --- Launcher commands -----------------------------------------------------

#[tauri::command]
fn launcher_state(app: AppHandle) -> serde_json::Value {
    let dir = libraries_dir(&app);
    let home = home_dir(&app).unwrap_or_default();
    let libraries: Vec<serde_json::Value> =
        launcher::libraries(&launcher::recents_file(&home), &dir)
            .into_iter()
            .map(|l| {
                // A library in the Datalib folder is known by its name; only
                // one somewhere else shows where it is.
                let elsewhere = l.path.parent() != Some(dir.as_path());
                serde_json::json!({
                    "name": launcher::display_name(&l.path),
                    "path": l.path.to_string_lossy(),
                    "shown_path": elsewhere.then(|| launcher::tilde(&l.path, &home)),
                    "found": l.found,
                    "summary": launcher::summary(&l.path),
                })
            })
            .collect();
    serde_json::json!({
        "libraries": libraries,
        "libraries_dir": launcher::tilde(&dir, &home),
        "suggested_name": launcher::suggested_name(&dir),
    })
}

/// What the new-library field names: the folder, as the popover shows
/// it, and what is there now. Null for an empty field.
#[tauri::command]
fn launcher_resolve(app: AppHandle, input: String) -> serde_json::Value {
    let home = home_dir(&app).unwrap_or_default();
    match launcher::resolve_new(&input, &home, &libraries_dir(&app)) {
        Some(root) => serde_json::json!({
            "shown_path": launcher::tilde(&root, &home),
            "path": root.to_string_lossy(),
            "target": launcher::classify(&root).as_str(),
        }),
        None => serde_json::Value::Null,
    }
}

/// Make the library the field names and open it on its Dashboard: the
/// starter config is written before the server starts (`--init`). A
/// folder that is already a library is opened instead.
#[tauri::command]
fn launcher_create(app: AppHandle, input: String) -> Result<(), String> {
    let home = home_dir(&app).unwrap_or_default();
    let root = launcher::resolve_new(&input, &home, &libraries_dir(&app))
        .ok_or("Type a name for the library, or a folder.")?;
    let shown = launcher::tilde(&root, &home);
    match launcher::classify(&root) {
        launcher::Target::Library => {
            tauri::async_runtime::spawn(boot(app, root, false));
        }
        launcher::Target::New => {
            std::fs::create_dir_all(&root).map_err(|e| format!("Could not create {shown}: {e}"))?;
            tauri::async_runtime::spawn(boot(app, root, true));
        }
        launcher::Target::Occupied => {
            return Err(format!(
                "{shown} has other files in it. Choose an empty folder, or a new name."
            ))
        }
        launcher::Target::NotAFolder => return Err(format!("{shown} is a file, not a folder.")),
    }
    Ok(())
}

/// The native folder picker, for the new-library field: the chosen
/// folder comes back as text for the field, and nothing is created
/// until Create.
#[tauri::command]
async fn launcher_choose_folder(app: AppHandle) -> Option<String> {
    let home = home_dir(&app).unwrap_or_default();
    app.dialog()
        .file()
        .set_title("Choose a folder for the new library")
        .blocking_pick_folder()
        .and_then(|choice| choice.into_path().ok())
        .map(|root| launcher::tilde(&root, &home))
}

/// Open a library the launcher listed. The path is checked rather than
/// trusted: the entry may have gone stale between render and click.
#[tauri::command]
fn launcher_open(app: AppHandle, path: String) -> Result<(), String> {
    let root = PathBuf::from(path);
    if !launcher::is_data_root(&root) {
        return Err(format!(
            "{} is no longer a data library — it may have been moved or deleted.",
            root.display()
        ));
    }
    tauri::async_runtime::spawn(boot(app, root, false));
    Ok(())
}

/// Open a library by picking its folder. Any folder is accepted: an
/// empty one gets the app's own first-run screen (see
/// `ui/src/views/FirstRunView.vue`). False when the picker was
/// canceled, so the page knows nothing is opening.
#[tauri::command]
async fn launcher_pick(app: AppHandle) -> Result<bool, String> {
    let Some(choice) = app
        .dialog()
        .file()
        .set_title("Open a library folder")
        .blocking_pick_folder()
    else {
        return Ok(false);
    };
    let root = choice
        .into_path()
        .map_err(|e| format!("unusable folder selection: {e}"))?;
    tauri::async_runtime::spawn(boot(app, root, false));
    Ok(true)
}

/// Take a library whose folder is gone off the list.
#[tauri::command]
fn launcher_forget(app: AppHandle, path: String) -> Result<(), String> {
    let home = home_dir(&app).ok_or("No home directory.")?;
    launcher::forget_recent(&launcher::recents_file(&home), Path::new(&path))
        .map_err(|e| e.to_string())
}

// --- The library menu, from the app's top bar -------------------------------

/// The open library and the others the menu offers.
#[tauri::command]
fn library_menu(app: AppHandle) -> serde_json::Value {
    let current = app
        .state::<DataRoot>()
        .0
        .lock()
        .expect("data root lock")
        .clone();
    let home = home_dir(&app).unwrap_or_default();
    let others: Vec<serde_json::Value> =
        launcher::libraries(&launcher::recents_file(&home), &libraries_dir(&app))
            .into_iter()
            .filter(|l| Some(&l.path) != current.as_ref())
            .map(|l| {
                serde_json::json!({
                    "name": launcher::display_name(&l.path),
                    "path": l.path.to_string_lossy(),
                    "found": l.found,
                })
            })
            .collect();
    serde_json::json!({
        "current": current.map(|p| p.to_string_lossy().into_owned()),
        "others": others,
    })
}

/// Close this library and open another.
#[tauri::command]
fn library_switch(app: AppHandle, path: String) -> Result<(), String> {
    let root = PathBuf::from(path);
    if !launcher::is_data_root(&root) {
        return Err(format!("{} is no longer a data library.", root.display()));
    }
    leave_library(&app, false).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn(boot(app, root, false));
    Ok(())
}

/// Close this library and go back to the libraries screen, with the
/// new-library form open when `new_library`.
#[tauri::command]
fn libraries_show(app: AppHandle, new_library: bool) -> Result<(), String> {
    leave_library(&app, new_library).map_err(|e| e.to_string())
}

/// Close the open library: its windows and its server. The launcher
/// opens first, because the app quits when its last window closes, and
/// it stays up through the next boot to report a failure.
fn leave_library(app: &AppHandle, new_library: bool) -> tauri::Result<()> {
    if app.get_webview_window(LAUNCHER_WINDOW).is_none() {
        show_launcher(app, new_library)?;
    }
    for (label, window) in app.webview_windows() {
        if label != LAUNCHER_WINDOW {
            let _ = window.destroy();
        }
    }
    let child = app
        .state::<HttpChild>()
        .0
        .lock()
        .expect("http child lock")
        .take();
    if let Some(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }
    *app.state::<DataRoot>().0.lock().expect("data root lock") = None;
    Ok(())
}

// --- Browse a raw store (src/raw_store.rs) ---------------------------------

/// Returns what the store was opened in, for the page to say.
#[tauri::command]
fn open_raw_store(app: AppHandle, path: String) -> Result<String, String> {
    let root = app
        .state::<DataRoot>()
        .0
        .lock()
        .expect("data root lock")
        .clone()
        .ok_or("No data library is open.")?;
    let store = raw_store::check_store(&root, Path::new(&path))?;
    match raw_store::choose(default_handler(&store)) {
        raw_store::Launch::DbBrowser { app: db_browser } => {
            spawn_open(&raw_store::db_browser_args(&db_browser, &store))?;
            Ok("DB Browser for SQLite".into())
        }
        raw_store::Launch::Shell => {
            let doltlite = resolve_bundled(&app, "datalib-doltlite", "DATALIB_DOLTLITE_BIN")
                .ok_or("The doltlite shell is not bundled with this app.")?;
            let script = write_shell_script(&raw_store::shell_script(&doltlite, &store))?;
            spawn_open(&[script.into()])?;
            Ok("a doltlite shell".into())
        }
    }
}

#[cfg(target_os = "macos")]
fn default_handler(file: &Path) -> Option<raw_store::Handler> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSBundle, NSString, NSURL};
    let url = NSURL::fileURLWithPath(&NSString::from_str(file.to_str()?));
    let app = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&url)?;
    Some(raw_store::Handler {
        app: PathBuf::from(app.path()?.to_string()),
        bundle_id: NSBundle::bundleWithURL(&app)
            .and_then(|b| b.bundleIdentifier())
            .map(|id| id.to_string()),
    })
}

#[cfg(not(target_os = "macos"))]
fn default_handler(_file: &Path) -> Option<raw_store::Handler> {
    None
}

/// A fresh name per click: Terminal may not have read the last script
/// yet, and each deletes itself once it runs.
fn write_shell_script(body: &str) -> Result<PathBuf, String> {
    static N: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "datalib-browse-{}-{}.command",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, body).map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(path)
}

#[cfg(target_os = "macos")]
fn spawn_open(args: &[std::ffi::OsString]) -> Result<(), String> {
    let out = Command::new("/usr/bin/open")
        .args(args)
        .output()
        .map_err(|e| format!("open: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!(
        "open failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

#[cfg(not(target_os = "macos"))]
fn spawn_open(_args: &[std::ffi::OsString]) -> Result<(), String> {
    Err("Opening a raw store is only built for macOS.".into())
}

fn home_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().home_dir().ok()
}

fn libraries_dir(app: &AppHandle) -> PathBuf {
    let documents = app
        .path()
        .document_dir()
        .ok()
        .or_else(|| Some(home_dir(app)?.join("Documents")))
        .unwrap_or_else(|| PathBuf::from("Documents"));
    launcher::libraries_dir(&documents)
}

fn main() {
    #[cfg(target_os = "macos")]
    inherit_shell_path();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Backs two things: the grid's "Reveal in Finder" action, and
        // handing an off-origin link to the OS browser (the `↗`
        // outlink and any link inside a rendered document). The webview
        // is granted those two commands and no others — notably not
        // `open_path`, which launches a local file's default
        // application. See `capabilities/reveal-local-files.json` and
        // `capabilities/open-external-urls.json`, which also explain
        // why both need a `remote` block at all (this app loads its UI
        // from localhost as an external URL, and Tauri withholds IPC
        // from remote origins by default).
        // Without `open_js_links_on_click(false)` the plugin injects a
        // click handler that sends every `target="_blank"` link to the OS
        // browser, same-origin included, so `on_new_window` never runs and
        // the card's ↗ lands in a browser with no session cookie.
        // Off-origin links are `ui/src/externalLinks.ts`'s job.
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            version,
            launcher_state,
            launcher_resolve,
            launcher_create,
            launcher_choose_folder,
            launcher_open,
            launcher_pick,
            launcher_forget,
            library_menu,
            library_switch,
            libraries_show,
            open_raw_store
        ])
        .manage(HttpChild(Mutex::new(None)))
        .manage(DataRoot(Mutex::new(None)))
        .setup(|app| {
            let handle = app.handle().clone();
            // A data root supplied non-interactively (positional arg or
            // `$DATALIB_DATA_ROOT`) skips the launcher and boots
            // straight into it — mirrors `datalib_http_bin <root>`
            // and makes the app scriptable/testable. Otherwise the
            // launcher window asks which library to open.
            match explicit_data_root() {
                Some(root) => {
                    tauri::async_runtime::spawn(boot(handle, root, false));
                }
                None => show_launcher(&handle, false)?,
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building datalib tauri app");

    app.run(|app, event| {
        // The backend child must not outlive the window: an orphaned
        // server would keep the doltlite file open and hold the port.
        if let tauri::RunEvent::Exit = event {
            let taken = app
                .state::<HttpChild>()
                .0
                .lock()
                .expect("http child lock")
                .take();
            if let Some(mut c) = taken {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    });
}

/// Apps launched from Finder/Dock inherit launchd's minimal PATH
/// (`/usr/bin:/bin:/usr/sbin:/sbin`), which lacks the Homebrew / nvm
/// directories where node and npx live. The backend normally runs
/// latchkey/qmd via the bundled runtime under `Resources/runtime/`
/// (see `datalib_core::node_runtime`) and doesn't need host node —
/// but its `npx` fallback (unstaged version pins, dev-ish setups) still
/// does. Capture the user's login-shell PATH so that fallback keeps
/// working (the spawned `datalib-http` child inherits it), the same
/// trick as the `fix-path-env` crate, without the extra dependency.
#[cfg(target_os = "macos")]
fn inherit_shell_path() {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let Ok(out) = std::process::Command::new(&shell)
        .args(["-lc", "printf %s \"$PATH\""])
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    if let Ok(path) = String::from_utf8(out.stdout) {
        let path = path.trim();
        if !path.is_empty() {
            std::env::set_var("PATH", path);
        }
    }
}

/// A data root supplied without the picker: first positional CLI arg,
/// else `$DATALIB_DATA_ROOT`. A leading `~` is expanded against
/// `$HOME` (same convention as `dev.sh`), since `open --env` and shell
/// exports don't do tilde expansion. Returns `None` when neither is set,
/// leaving the interactive picker as the default.
fn explicit_data_root() -> Option<PathBuf> {
    let raw = std::env::args()
        .nth(1)
        .filter(|a| !a.is_empty())
        .or_else(|| std::env::var("DATALIB_DATA_ROOT").ok())
        .filter(|a| !a.is_empty())?;
    let expanded = match raw.strip_prefix('~') {
        Some("") => std::env::var("HOME").unwrap_or(raw.clone()),
        Some(rest) if rest.starts_with('/') => {
            format!("{}{}", std::env::var("HOME").unwrap_or_default(), rest)
        }
        _ => raw,
    };
    Some(PathBuf::from(expanded))
}

/// `new_library` opens it with the new-library form showing.
fn show_launcher(app: &AppHandle, new_library: bool) -> tauri::Result<()> {
    let page = if new_library {
        "index.html#new"
    } else {
        "index.html"
    };
    under_title_bar(
        WebviewWindowBuilder::new(app, LAUNCHER_WINDOW, WebviewUrl::App(page.into()))
            .title("Data Liberation")
            .inner_size(1000.0, 720.0)
            .resizable(true),
    )
    .build()?;
    Ok(())
}

/// Label of the launcher window. Also named in
/// `capabilities/default.json`, which is what lets it call commands at
/// all — a window missing from every capability gets no IPC, and the
/// page's first `invoke` fails with nothing on screen to say why.
const LAUNCHER_WINDOW: &str = "launcher";

/// Locate a bundled binary. The dev override `$<env>` wins (point it at
/// a fresh Bazel build without rebundling); otherwise the copy bundled
/// under `Contents/Resources/binaries/` (see `tauri.conf.json`
/// `bundle.resources`), which `resource_dir()` resolves regardless of
/// where the bundle lives. The backend finds its own siblings there
/// (`binaries::resolve_binary_dir`), so only what the shell itself
/// runs is looked up here.
fn resolve_bundled(app: &AppHandle, name: &str, env: &str) -> Option<PathBuf> {
    if let Ok(p) = std::env::var(env) {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
        eprintln!("${env}={} is not a file", p.display());
    }
    let p = app.path().resource_dir().ok()?.join("binaries").join(name);
    p.is_file().then_some(p)
}

/// Start `root`'s server and open its window. `init` writes the starter
/// config first, for a library just created.
async fn boot(app: AppHandle, root: PathBuf, init: bool) {
    remember(&app, &root);
    let url = match tauri::async_runtime::spawn_blocking({
        let app = app.clone();
        move || start_backend(&app, root, init)
    })
    .await
    {
        Ok(Ok(url)) => url,
        Ok(Err(e)) => return boot_failed(&app, format!("could not start the backend: {e:#}")),
        Err(e) => return boot_failed(&app, format!("backend startup task panicked: {e}")),
    };
    let Ok(url) = url.parse::<Url>() else {
        return boot_failed(&app, format!("backend produced an unusable URL: {url}"));
    };
    // Serialized rather than kept as a `url::Origin`: that type is
    // not re-exported by tauri, and naming it would mean adding a
    // direct `url` dependency for one comparison.
    let app_origin = url.origin().ascii_serialization();
    let window = app_window(
        WebviewWindowBuilder::new(&app, "main", WebviewUrl::External(url))
            .title("Data Liberation")
            .inner_size(1280.0, 800.0),
        &app,
        &app_origin,
    )
    .build();
    if let Err(e) = window {
        return boot_failed(&app, format!("could not open the main window: {e}"));
    }
    // The app is up; the launcher has nothing left to offer. Closed
    // only here, at the end, so every failure above still has a window
    // to return to.
    if let Some(w) = app.get_webview_window(LAUNCHER_WINDOW) {
        let _ = w.close();
    }
}

/// A boot that did not produce a window.
fn boot_failed(app: &AppHandle, msg: String) {
    let Some(launcher) = app.get_webview_window(LAUNCHER_WINDOW) else {
        return fatal(app, msg);
    };
    eprintln!("{msg}");
    let _ = launcher.eval("location.reload()");
    app.dialog()
        .message(msg)
        .title("Datalib could not open that data library")
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}

/// Add `root` to the launcher's recent list. Best-effort: a home
/// directory we cannot resolve or write to costs the user a
/// convenience, never a launch.
fn remember(app: &AppHandle, root: &Path) {
    let Some(home) = home_dir(app) else { return };
    if let Err(e) = launcher::record_recent(&launcher::recents_file(&home), root) {
        eprintln!("could not record the recent data root: {e}");
    }
}

/// Windows the app opened on itself, numbered so their labels never
/// collide. `card-*` is what the capability files grant, so a window
/// opened this way can reveal files and pick paths like the main one.
static OPENED_WINDOWS: AtomicUsize = AtomicUsize::new(0);

/// On macOS the page draws its own toolbar where the title bar was:
/// the window's content runs under the bar, the title text is hidden,
/// and the three window buttons sit centred in the toolbar's 40px row.
/// The y is set by eye: the buttons' middle lands about 1pt above it.
/// The UI leaves them room and marks the toolbar as a drag region
/// (`App.vue`, capabilities/window-drag.json).
fn under_title_bar<'a>(
    builder: WebviewWindowBuilder<'a, Wry, AppHandle>,
) -> WebviewWindowBuilder<'a, Wry, AppHandle> {
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(16.0, 21.0));
    builder
}

/// The two rules every window of the app follows.
///
/// **A window shows the app, never someone else's website.** Rendered
/// documents carry links we did not author — the `↗` outlink, and every
/// `<a>` that came out of the source content (a "Sent via Superhuman"
/// footer, a newsletter's tracking link). Following one in place would
/// replace the whole UI with a marketing page and leave no chrome to
/// come back from, so an off-origin navigation goes to the OS browser.
///
/// **A `target="_blank"` link opens a window.** A webview has no tab
/// strip, so without this the card chrome's "open this card alone" ↗
/// and the grid's double-click were dead in the app. Same origin gets a
/// second window of the app, under the same rules; anything else goes
/// to the OS browser, as above.
fn app_window<'a>(
    builder: WebviewWindowBuilder<'a, Wry, AppHandle>,
    app: &AppHandle,
    app_origin: &str,
) -> WebviewWindowBuilder<'a, Wry, AppHandle> {
    let nav_app = app.clone();
    let nav_origin = app_origin.to_string();
    let new_app = app.clone();
    let new_origin = app_origin.to_string();
    under_title_bar(builder)
        .on_navigation(move |next| {
            if !leaves_the_app(next, &nav_origin) {
                return true;
            }
            open_externally(&nav_app, next);
            false
        })
        .on_new_window(move |url, features: NewWindowFeatures| {
            if leaves_the_app(&url, &new_origin) {
                open_externally(&new_app, &url);
                return NewWindowResponse::Deny;
            }
            let label = format!("card-{}", OPENED_WINDOWS.fetch_add(1, Ordering::Relaxed));
            // `about:blank`: the opener drives the load, as `window.open`
            // does in a browser. `window_features` hands the new webview
            // the opener's configuration, which macOS requires and which
            // is also what makes the two share the session cookie.
            let blank: Url = "about:blank".parse().expect("about:blank parses");
            let built = app_window(
                WebviewWindowBuilder::new(&new_app, &label, WebviewUrl::External(blank))
                    .title("Datalib")
                    .inner_size(1100.0, 760.0)
                    .window_features(features),
                &new_app,
                &new_origin,
            )
            .build();
            match built {
                Ok(window) => NewWindowResponse::Create { window },
                Err(e) => {
                    eprintln!("could not open a window for {url}: {e}");
                    NewWindowResponse::Deny
                }
            }
        })
}

fn open_externally(app: &AppHandle, url: &Url) {
    if let Err(e) = app.opener().open_url(url.as_str(), None::<&str>) {
        eprintln!("could not open {url} externally: {e}");
    }
}

fn leaves_the_app(next: &Url, app_origin: &str) -> bool {
    match next.scheme() {
        "http" | "https" => next.origin().ascii_serialization() != app_origin,
        "mailto" | "tel" => true,
        _ => false,
    }
}

/// Spawn the bundled `datalib-http` against `root` on an ephemeral
/// localhost port and wait (≤15s) for it to announce its URL via
/// `--url-file`. The child's output goes to a log file in the temp dir
/// so startup failures can quote it in the error dialog (a
/// Finder-launched app has no terminal). Blocking: run on a worker
/// thread, not the event loop.
fn start_backend(app: &AppHandle, root: PathBuf, init: bool) -> anyhow::Result<String> {
    let http_bin = resolve_bundled(app, "datalib-http", "DATALIB_HTTP_BIN").ok_or_else(|| {
        anyhow::anyhow!(
            "datalib-http binary not found (no bundled copy and \
             $DATALIB_HTTP_BIN not set)"
        )
    })?;

    let tmp = std::env::temp_dir();
    let pid = std::process::id();
    let url_file = tmp.join(format!("datalib-http-{pid}.url"));
    let log_file = tmp.join(format!("datalib-http-{pid}.log"));
    // Remove a stale url-file from a recycled PID so we can't read a
    // dead server's address.
    let _ = std::fs::remove_file(&url_file);

    let log = std::fs::File::create(&log_file)
        .map_err(|e| anyhow::anyhow!("create backend log {}: {e}", log_file.display()))?;
    // The backend announces its launch URL — which carries the API
    // token — on stderr, and that lands in this file, in a temp dir
    // that is shared between users on Linux. Owner-only. (The url-file
    // itself is tightened by the backend for the same reason.)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&log_file, std::fs::Permissions::from_mode(0o600));
    }
    let log_err = log
        .try_clone()
        .map_err(|e| anyhow::anyhow!("clone backend log handle: {e}"))?;

    let mut child = Command::new(&http_bin)
        .arg(&root)
        .arg("--no-open")
        .arg("--url-file")
        .arg(&url_file)
        .args(init.then_some("--init"))
        .env("DATALIB_BIND", "127.0.0.1:0")
        // The backend exits when this pipe hits EOF, which the kernel
        // arranges however the shell goes — the `kill` at exit is for
        // the ways it can still run code, this is for the ones it
        // can't. `child` keeps the write end; never `take()` it.
        .env("DATALIB_PARENT_PIPE", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()
        .map_err(|e| anyhow::anyhow!("spawn {}: {e}", http_bin.display()))?;

    // Poll for the URL announcement, watching for an early child death
    // so a bad data root fails with the backend's own message instead
    // of a timeout.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let url = loop {
        if let Ok(url) = std::fs::read_to_string(&url_file) {
            let url = url.trim().to_string();
            if !url.is_empty() {
                break url;
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            anyhow::bail!(
                "datalib-http exited during startup ({status}):\n{}",
                log_tail(&log_file)
            );
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "datalib-http did not announce its URL within 15s \
                 (log: {}):\n{}",
                log_file.display(),
                log_tail(&log_file)
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let _ = std::fs::remove_file(&url_file);

    *app.state::<HttpChild>().0.lock().expect("http child lock") = Some(child);
    *app.state::<DataRoot>().0.lock().expect("data root lock") = Some(root);
    Ok(url)
}

fn log_tail(path: &std::path::Path) -> String {
    let Ok(content) = std::fs::read_to_string(path) else {
        return String::from("(no backend log captured)");
    };
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(20);
    lines[start..].join("\n")
}

fn fatal(app: &AppHandle, msg: String) {
    eprintln!("{msg}");
    let handle = app.clone();
    app.dialog()
        .message(msg)
        .title("Datalib failed to start")
        .kind(MessageDialogKind::Error)
        .show(move |_| handle.exit(1));
}
