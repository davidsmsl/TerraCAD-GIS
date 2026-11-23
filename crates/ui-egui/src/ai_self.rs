//! The assistant changing the application itself, and saving that work.
//!
//! A reply of the AI assistant may carry a unified diff for the application's source code. The person reviews it
//! (the files it touches), and only then is it applied to the source folder as a git commit of its own. A change
//! needs the program rebuilt and restarted to take effect, so the window shows a banner with the choices
//! "rebuild and restart" or "later"; the list of waiting changes is kept in a file, so the banner is still there
//! in the next session. A GitHub repository and token can be connected so progress is pushed to a branch.
//!
//! Nothing here acts without a button press, and nothing here writes a token to a log, a message or a status line.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

use cadcraft_civil::ai::{AiError, Settings, TestReport};
use cadcraft_civil::selfmod::{self, GitHub, Pending};
use egui::RichText;

use crate::CadApp;
use crate::theme::Tokens;

/// A patch waiting for the person's confirmation.
pub struct Confirm {
    pub patch: String,
    pub title: String,
    /// git's summary of what it changes, or why it cannot be applied.
    pub stat: Result<String, String>,
}

/// What the banner asks the window to do.
pub enum Act {
    Build,
    RestartBuilt(PathBuf),
    /// Restart this program as it is (settings, plugins and layouts are read again).
    Restart,
    /// The changes were built outside; forget the reminder.
    Dismiss,
}

#[derive(Default)]
pub struct State {
    loaded: bool,
    /// Source changes applied since the program was built.
    pub pending: Pending,
    pub gh: GitHub,
    /// Push to GitHub after each change is applied.
    pub auto_save: bool,
    ai_test: Option<Receiver<TestReport>>,
    pub ai_report: Option<TestReport>,
    gh_test: Option<Receiver<Result<String, AiError>>>,
    push: Option<Receiver<Result<String, AiError>>>,
    /// The last GitHub answer: `Ok` text or `Err` text.
    pub gh_report: Option<Result<String, String>>,
    build: Option<(Receiver<Result<PathBuf, AiError>>, Instant)>,
    /// A program rebuilt from the changes, ready to start.
    pub built: Option<PathBuf>,
    pub build_error: Option<String>,
    pub confirm: Option<Confirm>,
}

impl State {
    pub fn busy_ai_test(&self) -> bool {
        self.ai_test.is_some()
    }
    pub fn busy_github(&self) -> bool {
        self.gh_test.is_some() || self.push.is_some()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn settings_dir() -> Option<PathBuf> {
    cadcraft_engine::settings_dir::current()
}

#[cfg(target_arch = "wasm32")]
fn settings_dir() -> Option<PathBuf> {
    None
}

fn pending_file() -> Option<PathBuf> {
    settings_dir().map(|d| d.join("assistant-pending.json"))
}

fn build_log() -> PathBuf {
    settings_dir().unwrap_or_else(std::env::temp_dir).join("assistant-build.log")
}

/// Read what was left waiting by an earlier session (once).
pub fn ensure_loaded(st: &mut State) {
    if !st.loaded {
        st.loaded = true;
        if let Some(f) = pending_file() {
            st.pending = Pending::load(&f);
        }
    }
}

fn persist_pending(st: &State) {
    if let Some(f) = pending_file() {
        let _ = st.pending.save(&f);
    }
}

/// Run `job` on a worker thread and repaint when it ends. `false` when threads are not available (web).
#[cfg(not(target_arch = "wasm32"))]
fn spawn(name: &str, ctx: &egui::Context, job: impl FnOnce() + Send + 'static) -> bool {
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            job();
            ctx.request_repaint();
        })
        .is_ok()
}

#[cfg(target_arch = "wasm32")]
fn spawn(_: &str, _: &egui::Context, _: impl FnOnce() + Send + 'static) -> bool {
    false
}

/// Test the assistant's connection in the background.
pub fn start_ai_test(st: &mut State, settings: Settings, ctx: &egui::Context) {
    if st.ai_test.is_some() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    st.ai_report = None;
    if spawn("civil-ai-test", ctx, move || {
        let _ = tx.send(cadcraft_civil::ai::test_connection(&settings));
    }) {
        st.ai_test = Some(rx);
    } else {
        st.ai_report = Some(TestReport { ok: false, lines: vec![format!("✘ {}", crate::tl!("Could not start the request."))] });
    }
}

/// Test the GitHub token and repository in the background.
pub fn start_github_test(st: &mut State, ctx: &egui::Context) {
    if st.busy_github() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    st.gh_report = None;
    let gh = st.gh.clone();
    if spawn("github-test", ctx, move || {
        let _ = tx.send(selfmod::test_github(&gh));
    }) {
        st.gh_test = Some(rx);
    } else {
        st.gh_report = Some(Err(crate::tl!("Could not start the request.").to_string()));
    }
}

/// Commit what changed in the source folder and push it to the connected repository, in the background.
pub fn start_push(st: &mut State, root: Option<PathBuf>, message: String, ctx: &egui::Context) {
    if st.busy_github() {
        return;
    }
    let Some(root) = root else {
        st.gh_report = Some(Err(crate::tl!("The source code folder was not found. Set it in AI Assistant > Source Code.").to_string()));
        return;
    };
    let (tx, rx) = std::sync::mpsc::channel();
    st.gh_report = None;
    let gh = st.gh.clone();
    if spawn("github-push", ctx, move || {
        let _ = tx.send(selfmod::save_progress(&root, &gh, &message));
    }) {
        st.push = Some(rx);
    } else {
        st.gh_report = Some(Err(crate::tl!("Could not start the request.").to_string()));
    }
}

/// Rebuild the program from the source folder, in the background (it takes minutes).
pub fn start_build(st: &mut State, root: Option<PathBuf>, ctx: &egui::Context) {
    if st.build.is_some() {
        return;
    }
    let Some(root) = root else {
        st.build_error = Some(crate::tl!("The source code folder was not found. Set it in AI Assistant > Source Code.").to_string());
        return;
    };
    st.build_error = None;
    st.built = None;
    let (tx, rx) = std::sync::mpsc::channel();
    let log = build_log();
    if spawn("assistant-build", ctx, move || {
        let _ = tx.send(selfmod::rebuild(&root, &log));
    }) {
        st.build = Some((rx, Instant::now()));
    } else {
        st.build_error = Some(crate::tl!("Could not start the request.").to_string());
    }
}

/// Take the answers of the background jobs that have finished.
pub fn poll(st: &mut State) {
    if let Some(rx) = &st.ai_test {
        match rx.try_recv() {
            Ok(r) => {
                st.ai_report = Some(r);
                st.ai_test = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                st.ai_report = Some(TestReport { ok: false, lines: vec!["✘ the test ended without an answer".into()] });
                st.ai_test = None;
            }
        }
    }
    for which in 0..2 {
        let rx = if which == 0 { &st.gh_test } else { &st.push };
        let Some(rx) = rx else { continue };
        let answer = match rx.try_recv() {
            Ok(a) => a,
            Err(TryRecvError::Empty) => continue,
            Err(TryRecvError::Disconnected) => Err(AiError("the request ended without an answer".into())),
        };
        st.gh_report = Some(answer.map_err(|e| e.0));
        if which == 0 {
            st.gh_test = None;
        } else {
            st.push = None;
        }
    }
    if let Some((rx, _)) = &st.build {
        let answer = match rx.try_recv() {
            Ok(a) => Some(a),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(AiError("the build ended without an answer".into()))),
        };
        if let Some(a) = answer {
            st.build = None;
            match a {
                Ok(exe) => st.built = Some(exe),
                Err(e) => st.build_error = Some(e.0),
            }
        }
    }
}

/// Look at a patch: what it touches, and whether it applies. Waits for the person's confirmation.
pub fn review(st: &mut State, root: Option<&Path>, patch: &str) {
    let stat = match root {
        None => Err(crate::tl!("The source code folder was not found. Set it in AI Assistant > Source Code.").to_string()),
        Some(r) => selfmod::check_patch(r, patch).map_err(|e| e.0),
    };
    st.confirm = Some(Confirm { patch: patch.to_string(), title: crate::tl!("Change proposed by the assistant").to_string(), stat });
}

/// Apply the confirmed patch and remember it as waiting for a rebuild. Returns a line for the person.
pub fn apply_confirmed(st: &mut State, root: Option<&Path>, ctx: &egui::Context) -> Result<String, String> {
    let Some(c) = st.confirm.take() else { return Err(String::new()) };
    let Some(root) = root else { return Err(crate::tl!("The source code folder was not found. Set it in AI Assistant > Source Code.").to_string()) };
    match selfmod::apply_patch(root, &c.patch, &c.title) {
        Ok(change) => {
            let line = crate::tf!(
                "Applied and saved as commit {commit}: {n} files. Rebuild and restart to use it.",
                commit = change.commit,
                n = change.files.len()
            );
            st.pending.changes.push(change);
            persist_pending(st);
            st.built = None;
            if st.auto_save && st.gh.validate().is_ok() {
                let message = st.pending.changes.last().map(|c| c.title.clone()).unwrap_or_default();
                start_push(st, Some(root.to_path_buf()), message, ctx);
            }
            Ok(line)
        }
        Err(e) => {
            let msg = e.0;
            st.confirm = Some(Confirm { stat: Err(msg.clone()), ..c });
            Err(msg)
        }
    }
}

/// The banner about changes waiting for a rebuild and a restart.
pub fn banner(ui: &mut egui::Ui, st: &State) -> Option<Act> {
    if st.pending.changes.is_empty() && st.built.is_none() {
        return None;
    }
    let t = Tokens::get();
    let mut act = None;
    ui.group(|ui| {
        ui.set_width(ui.available_width());
        if !st.pending.changes.is_empty() {
            ui.label(
                RichText::new(crate::tf!(
                    "{n} change(s) applied to the source code are not in this running program yet.",
                    n = st.pending.changes.len()
                ))
                .strong(),
            );
            for c in st.pending.changes.iter().rev().take(4) {
                ui.label(RichText::new(format!("• {} ({})", c.title, c.commit)).small());
            }
        }
        if let Some((_, started)) = &st.build {
            let s = started.elapsed().as_secs();
            ui.label(crate::tf!("Building... {m}:{s} (it can take several minutes; you can keep working)", m = s / 60, s = format!("{:02}", s % 60)));
            ui.add(egui::ProgressBar::new(0.0).animate(true).desired_width(220.0));
        }
        if let Some(e) = &st.build_error {
            ui.colored_label(t.warn, e);
        }
        ui.horizontal_wrapped(|ui| {
            if let Some(exe) = &st.built {
                ui.label(RichText::new(crate::tl!("The new version is built.")).color(t.accent));
                if ui.button(crate::tl!("Restart now with the new version")).clicked() {
                    act = Some(Act::RestartBuilt(exe.clone()));
                }
            } else if st.build.is_none() {
                if ui
                    .button(crate::tl!("Rebuild and restart"))
                    .on_hover_text(crate::tl!("Compiles the source code (needs Rust installed) and then asks to restart."))
                    .clicked()
                {
                    act = Some(Act::Build);
                }
                if ui.button(crate::tl!("Restart without rebuilding")).clicked() {
                    act = Some(Act::Restart);
                }
            }
            if st.build.is_none() {
                // Closing the window is enough: the reminder is kept for the next session.
                ui.label(RichText::new(crate::tl!("Or leave it for later: the reminder stays for the next session.")).small());
            }
            if st.build.is_none()
                && !st.pending.changes.is_empty()
                && ui.button(crate::tl!("I built it already")).on_hover_text(crate::tl!("Forgets the reminder.")).clicked()
            {
                act = Some(Act::Dismiss);
            }
        });
    });
    act
}

/// Carry out what the banner asked.
pub fn run_act(app: &mut CadApp, act: Act, root: Option<PathBuf>, ctx: &egui::Context) {
    match act {
        Act::Build => start_build(&mut app.civil_ai.selfmod, root, ctx),
        Act::RestartBuilt(exe) => {
            // The changes are in this program now: the reminder goes.
            app.civil_ai.selfmod.pending.changes.clear();
            persist_pending(&app.civil_ai.selfmod);
            app.civil_ai.selfmod.built = None;
            restart(app, exe);
        }
        Act::Restart => match std::env::current_exe() {
            Ok(exe) => restart(app, exe),
            Err(e) => app.set_status(e.to_string()),
        },
        Act::Dismiss => {
            app.civil_ai.selfmod.pending.changes.clear();
            persist_pending(&app.civil_ai.selfmod);
        }
    }
}

/// Close the program (asking about unsaved drawings first) and start `exe` once it has closed. If the person
/// cancels the question, nothing restarts.
pub fn restart(app: &mut CadApp, exe: PathBuf) {
    app.restart_exe = Some(exe);
    if !app.request_quit() {
        app.quit_confirmed = true;
        app.quit_requested = true;
    }
}

static RELAUNCH: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The program to start after the window has closed (called when the close really happens).
pub fn set_relaunch(exe: PathBuf) {
    if let Ok(mut g) = RELAUNCH.lock() {
        *g = Some(exe);
    }
}

/// Start the program set by [`set_relaunch`], with the arguments this one had. Call after the event loop ends.
pub fn relaunch_if_requested() {
    let exe = RELAUNCH.lock().ok().and_then(|mut g| g.take());
    if let Some(exe) = exe {
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(std::env::args_os().skip(1));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0000_0008); // DETACHED_PROCESS
        }
        if let Err(e) = cmd.spawn() {
            eprintln!("TerraCAD-GIS: no se pudo reiniciar {}: {e}", exe.display());
        }
    }
}

/// The result of a connection test, one line per step.
pub fn report_ui(ui: &mut egui::Ui, report: &TestReport) {
    let t = Tokens::get();
    for line in &report.lines {
        let color = if line.starts_with('✘') {
            t.warn
        } else if line.starts_with('✔') {
            t.accent
        } else {
            t.text_dim
        };
        ui.label(RichText::new(line.as_str()).small().color(color));
    }
}

/// The confirmation shown before a patch is applied. Returns `Some(true)` to apply, `Some(false)` to cancel.
pub fn confirm_ui(ui: &mut egui::Ui, st: &mut State) -> Option<bool> {
    let c = st.confirm.as_mut()?;
    let t = Tokens::get();
    let mut answer = None;
    ui.group(|ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(crate::tl!("Apply this change to the source code?")).strong());
        match &c.stat {
            Ok(stat) => {
                egui::ScrollArea::vertical().id_salt("selfmod_stat").max_height(110.0).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(stat.as_str()).monospace().small()).wrap());
                });
                ui.label(RichText::new(crate::tl!("It is saved as a git commit of its own, so it can be undone with git revert.")).small());
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("Description"));
                    ui.add(egui::TextEdit::singleline(&mut c.title).desired_width(260.0));
                });
            }
            Err(e) => {
                ui.colored_label(t.warn, e.as_str());
            }
        }
        ui.horizontal(|ui| {
            if c.stat.is_ok() && ui.button(crate::tl!("Apply and save commit")).clicked() {
                answer = Some(true);
            }
            if ui.button(crate::tl!("Cancel")).clicked() {
                answer = Some(false);
            }
        });
    });
    answer
}

/// The GitHub part of the settings: repository, branch, token, and the buttons that use them.
/// Returns the buttons pressed: `(test, save now)`.
pub fn github_ui(ui: &mut egui::Ui, st: &mut State) -> (bool, bool) {
    let (mut test, mut save) = (false, false);
    ui.separator();
    ui.label(RichText::new(crate::tl!("GitHub (saves the source code progress)")).strong());
    ui.horizontal(|ui| {
        ui.label(crate::tl!("Repository"));
        ui.add(egui::TextEdit::singleline(&mut st.gh.repo).hint_text("owner/repository").desired_width(200.0));
        ui.label(crate::tl!("Branch"));
        ui.add(egui::TextEdit::singleline(&mut st.gh.branch).hint_text(selfmod::DEFAULT_BRANCH).desired_width(170.0));
    });
    ui.horizontal(|ui| {
        ui.label(crate::tl!("Token"));
        ui.add(egui::TextEdit::singleline(&mut st.gh.token).password(true).hint_text("github_pat_…").desired_width(280.0));
    });
    ui.hyperlink_to(
        crate::tl!("Create a token (Contents: Read and write on that repository)"),
        "https://github.com/settings/personal-access-tokens/new",
    );
    ui.checkbox(&mut st.auto_save, crate::tl!("Save to GitHub after each change the assistant applies"));
    ui.horizontal(|ui| {
        let ready = st.gh.validate().is_ok() && !st.busy_github();
        if ui.add_enabled(ready, egui::Button::new(crate::tl!("Test GitHub"))).clicked() {
            test = true;
        }
        if ui.add_enabled(ready, egui::Button::new(crate::tl!("Save progress now"))).clicked() {
            save = true;
        }
        if st.busy_github() {
            ui.spinner();
        }
    });
    if let Some(r) = &st.gh_report {
        let t = Tokens::get();
        match r {
            Ok(m) => ui.label(RichText::new(format!("✔ {m}")).small().color(t.accent)),
            Err(e) => ui.label(RichText::new(format!("✘ {e}")).small().color(t.warn)),
        };
    }
    (test, save)
}

/// The token to start with: what was saved, or the environment.
pub fn token_from_env() -> String {
    ["TERRACAD_GITHUB_TOKEN", "GITHUB_TOKEN"].iter().find_map(|v| std::env::var(v).ok().filter(|t| !t.trim().is_empty())).unwrap_or_default()
}
