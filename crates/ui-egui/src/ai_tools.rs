//! The assistant's tools: the procedure library, the temporary toolbar, trial runs on a copy of
//! the drawing, macro recording and the source-code package for outside tools.
//!
//! Nothing here runs by itself. A procedure runs when the person presses its button; a trial run
//! is undone as soon as it ends; the patch that would make a procedure part of the application
//! is text to review and apply, never applied by the program. The steps run through the same
//! registered commands as the menus and the automation interface ([`cadcraft_engine::Session::execute`]).

use std::path::PathBuf;

use cadcraft_civil::plugins::{self, Installed, Plugin};
use cadcraft_civil::procedures::{self, Library, Procedure, Step};
use cadcraft_civil::source;
use egui::{RichText, vec2};
use serde_json::Value;

use crate::CadApp;
use crate::theme::Tokens;

/// What a trial run found, step by step.
pub struct Trial {
    pub name: String,
    pub results: Vec<(String, Result<String, String>)>,
    pub before: usize,
    pub after: usize,
}

/// A procedure being written or edited.
#[derive(Clone, Default)]
struct Draft {
    /// The id being replaced when editing a saved procedure.
    editing: Option<String>,
    name: String,
    description: String,
    icon: String,
    toolbar: bool,
    /// The steps as editable JSON text: `[{"command": ..., "params": {...}}]`.
    steps: String,
    notes: String,
}

#[derive(Default)]
pub struct State {
    loaded: bool,
    lib: Library,
    bundled: Vec<Procedure>,
    /// The assistant's toolbar is shown.
    pub toolbar: bool,
    /// Selected procedure id in the library window.
    selected: Option<String>,
    draft: Option<Draft>,
    note: String,
    pub trial: Option<Trial>,
    /// Patch text for the selected procedure, and where it was saved.
    patch: String,
    /// The source folder the person set ("" = detect).
    pub source_dir: String,
    /// Send the source code with each question.
    pub use_source: bool,
    package_request: String,
    /// A question to put in the chat box the next time the assistant opens.
    pub prefill: Option<String>,
    /// The assistant window should open (set by recording and trials).
    pub open_chat: bool,
    /// Installed plugins ([`plugins`]), read when first needed.
    pub plugins: Installed,
    plugins_loaded: bool,
    /// Startup LISP of the allowed plugins has run this session.
    startup_done: bool,
    /// Plugin toolbars the person closed this session.
    closed_bars: Vec<String>,
    /// A plugin file waiting for the person's review before it is installed.
    review: Option<Plugin>,
    /// Fields of "create a plugin from my procedures".
    export_name: String,
    export_menu: String,
    export_pick: Vec<String>,
}

fn plugins_dir() -> Option<PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        cadcraft_engine::settings_dir::current().map(|d| d.join("plugins"))
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

fn command_exists(c: &str) -> bool {
    cadcraft_engine::find_command(c).is_some()
}

/// Read the installed plugins, once (and again after an install, removal or switch).
pub fn ensure_plugins(st: &mut State) {
    if !st.plugins_loaded {
        reload_plugins(st);
    }
}

fn reload_plugins(st: &mut State) {
    st.plugins_loaded = true;
    st.plugins = plugins_dir().map(|d| Installed::load(&d, command_exists)).unwrap_or_default();
}

/// The menus the active plugins add: title and items `(label, id)`, same-title menus merged.
pub fn plugin_menus(st: &State, reserved: &[&str]) -> Vec<(String, Vec<(String, String)>)> {
    let mut out: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for p in st.plugins.active() {
        let Some(title) = p.menu_title(reserved) else { continue };
        let items = p.items.iter().map(|i| (i.label.clone(), format!("plugin.run:{}:{}", p.id, i.procedure)));
        match out.iter_mut().find(|(t, _)| *t == title) {
            Some((_, v)) => v.extend(items),
            None => out.push((title, items.collect())),
        }
    }
    out
}

/// Run the plugin procedure a menu item named (`plugin.run:<plugin>:<procedure>`). `None` when `id` is not one.
pub fn run_plugin_item(app: &mut CadApp, id: &str) -> Option<Result<Value, String>> {
    let rest = id.strip_prefix("plugin.run:")?;
    let (plugin, procedure) = rest.split_once(':')?;
    ensure_plugins(&mut app.civil_ai.tools);
    let found = app.civil_ai.tools.plugins.active().find(|p| p.id == plugin).and_then(|p| p.procedure(procedure)).cloned();
    let Some(p) = found else { return Some(Err(format!("plugin item {rest} not found or switched off"))) };
    Some(run_procedure(app, &p))
}

/// Run a procedure's steps and say how it went, in the status bar and in the result.
pub fn run_procedure(app: &mut CadApp, p: &Procedure) -> Result<Value, String> {
    let results = run_steps(app, &p.steps);
    match results.iter().find_map(|(s, r)| r.as_ref().err().map(|e| format!("{s}: {e}"))) {
        Some(e) => {
            app.set_status(&e);
            Err(e)
        }
        None => {
            app.set_status(crate::tf!("{name}: done.", name = p.name));
            Ok(serde_json::json!({ "steps": results.len() }))
        }
    }
}

/// Run the startup LISP of the plugins the person allowed, once per session, when a drawing is open.
pub fn startup(app: &mut CadApp) {
    ensure_plugins(&mut app.civil_ai.tools);
    if app.civil_ai.tools.startup_done || app.session.doc().is_err() {
        return;
    }
    app.civil_ai.tools.startup_done = true;
    let st = &app.civil_ai.tools;
    let files: Vec<(String, String)> = st
        .plugins
        .active()
        .filter(|p| st.plugins.state.may_run_startup(&p.id))
        .flat_map(|p| p.lisp.iter().map(|l| (format!("{}/{}", p.id, l.name), l.code.clone())))
        .collect();
    for (name, code) in files {
        if let Err(e) = app.session.execute("lisp", &serde_json::json!({ "code": code })) {
            app.session.echo(format!("Plugin {name}: {e}"));
        }
    }
}

/// Read a plugin file and put it up for review (the person decides to install it).
pub fn review_file(app: &mut CadApp, path: &str) {
    let st = &mut app.civil_ai.tools;
    let text = std::fs::metadata(path).map_err(|e| e.to_string()).and_then(|m| {
        if m.len() > plugins::MAX_BYTES {
            Err(crate::tl!("That file is too large to be a plugin.").to_string())
        } else {
            std::fs::read_to_string(path).map_err(|e| e.to_string())
        }
    });
    match text.and_then(|t| plugins::parse(&t)) {
        Ok(p) => {
            st.review = Some(p);
            st.note.clear();
        }
        Err(e) => st.note = e,
    }
    app.ui.dialog = Some("ai_plugins".into());
}

fn library_file() -> Option<PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        cadcraft_engine::settings_dir::current().map(|d| d.join("ai-procedures.json"))
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

fn load(st: &mut State) {
    st.loaded = true;
    st.bundled = procedures::bundled();
    match library_file() {
        Some(f) => match Library::load(&f) {
            Ok(l) => st.lib = l,
            Err(e) => st.note = e,
        },
        None => st.note = crate::tl!("There is no settings folder to save into.").to_string(),
    }
}

fn persist(st: &mut State) {
    st.note = match library_file() {
        Some(f) => st.lib.save(&f).map_or_else(|e| e, |()| crate::tl!("Saved.").to_string()),
        None => crate::tl!("Saving is not available in the web version; copy the text instead.").to_string(),
    };
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn find(st: &State, id: &str) -> Option<Procedure> {
    st.lib.get(id).cloned().or_else(|| st.bundled.iter().find(|p| p.id == id).cloned())
}

/// Every procedure, the person's first.
fn all(st: &State) -> Vec<Procedure> {
    st.lib.procedures.iter().chain(st.bundled.iter()).cloned().collect()
}

/// Start a draft from steps the assistant proposed or a recording became.
pub fn draft_from_steps(app: &mut CadApp, steps: &[Step], name: &str) {
    let st = &mut app.civil_ai.tools;
    if !st.loaded {
        load(st);
    }
    st.draft = Some(Draft {
        name: name.to_string(),
        icon: name.chars().filter(|c| c.is_alphanumeric()).take(3).collect::<String>().to_uppercase(),
        toolbar: true,
        steps: serde_json::to_string_pretty(&steps_json(steps)).unwrap_or_default(),
        ..Draft::default()
    });
    app.ui.dialog = Some("ai_procedures".into());
}

fn steps_json(steps: &[Step]) -> Value {
    serde_json::to_value(steps).unwrap_or(Value::Null)
}

/// Run `steps` on the drawing as they are, saying what each answered.
pub fn run_steps(app: &mut CadApp, steps: &[Step]) -> Vec<(String, Result<String, String>)> {
    let mut out = Vec::new();
    for s in steps {
        let r = match app.session.execute(&s.command, &s.params) {
            Ok(v) => Ok(v.get("message").and_then(Value::as_str).map_or_else(|| crate::tl!("Done.").to_string(), str::to_string)),
            Err(e) => Err(e.to_string()),
        };
        let failed = r.is_err();
        out.push((s.summary(), r));
        if failed {
            break;
        }
    }
    out
}

/// Run `steps` on the drawing and take it all back: the drawing, selection and undo steps are
/// restored from a checkpoint. The report goes to the chat box so the assistant can correct it.
pub fn trial(app: &mut CadApp, name: &str, steps: &[Step]) {
    let checkpoint = match app.session.checkpoint() {
        Ok(c) => c,
        Err(e) => {
            app.civil_ai.tools.note = e.to_string();
            return;
        }
    };
    let before = checkpoint.entities();
    let results = run_steps(app, steps);
    let after = app.session.entity_count();
    app.session.restore(&checkpoint);
    let report = procedures::test_report_prompt(name, &results, before, after);
    let st = &mut app.civil_ai.tools;
    st.prefill = Some(report);
    st.note = crate::tl!("Trial run finished and undone. The report is ready in the assistant.").to_string();
    st.trial = Some(Trial { name: name.to_string(), results, before, after });
}

/// Begin recording what the person does.
pub fn start_recording(app: &mut CadApp) {
    app.session.start_recording();
    app.set_status(crate::tl!("Recording. Do the task, then choose AI Assistant > Stop and Analyze Recording."));
}

/// End the recording and put the analysis request in the assistant's chat box.
pub fn stop_recording(app: &mut CadApp) {
    if !app.session.is_recording() {
        app.set_status(crate::tl!("There is no recording in progress."));
        return;
    }
    let lines = app.session.stop_recording();
    if lines.is_empty() {
        app.set_status(crate::tl!("The recording is empty."));
        return;
    }
    let st = &mut app.civil_ai.tools;
    st.prefill = Some(procedures::recording_prompt(&lines));
    st.open_chat = true;
    app.ui.dialog = Some("civil_ai".into());
}

/// The source folder to use, when there is one.
pub fn source_root(st: &State) -> Option<PathBuf> {
    source::find_root(&st.source_dir)
}

/// The source-code context to send with `question`, when the person asked for it.
pub fn context_for(st: &State, question: &str) -> String {
    if !st.use_source {
        return String::new();
    }
    source_root(st).map(|r| source::prompt_context(&r, question, 60_000)).unwrap_or_default()
}

fn write_in_settings(sub: &str, name: &str, text: &str) -> Result<PathBuf, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let dir = cadcraft_engine::settings_dir::current()
            .map(|d| d.join(sub))
            .ok_or_else(|| crate::tl!("There is no settings folder to save into.").to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(name);
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        Ok(path)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (sub, name, text);
        Err(crate::tl!("Saving is not available in the web version; copy the text instead.").to_string())
    }
}

/// The patch that would add `p` to the application, from the source folder's bundle.
fn patch_for(st: &State, p: &Procedure) -> Result<String, String> {
    let root =
        source_root(st).ok_or_else(|| crate::tl!("The source code folder was not found. Set it in AI Assistant > Source Code.").to_string())?;
    let current = std::fs::read_to_string(root.join(procedures::BUNDLE_PATH)).unwrap_or_default();
    procedures::promotion_patch(p, &current)
}

/// The assistant's bar (recording, the assistant, the library and the procedures marked for the
/// toolbar) and one bar per active plugin that has buttons, as dockable bars: `(id, title, items,
/// closable)`. Their clicks come back through [`run_action`].
pub fn bars(app: &mut CadApp) -> Vec<(String, String, Vec<crate::bars::Item>, bool)> {
    use crate::bars::{Glyph, Item};
    let mut out = Vec::new();
    ensure_plugins(&mut app.civil_ai.tools);
    if app.civil_ai.tools.toolbar {
        if !app.civil_ai.tools.loaded {
            load(&mut app.civil_ai.tools);
        }
        let recording = app.session.is_recording();
        let n = app.session.recorded_lines();
        let text = |s: &str, tip: String, action: &str| Item { glyph: Glyph::Text(s.to_string()), tip, action: action.to_string(), enabled: true };
        let rec_tip =
            if recording { format!("{} ({n})", crate::tl!("Stop and Analyze Recording")) } else { crate::tl!("Start Recording").to_string() };
        let mut items = vec![
            text("AI", crate::tl!("AI Assistant").to_string(), "ai:chat"),
            text(if recording { "■" } else { "●" }, rec_tip, "ai:rec"),
            text("≡", crate::tl!("Procedure Library").to_string(), "ai:lib"),
        ];
        for p in all(&app.civil_ai.tools).into_iter().filter(|p| p.toolbar) {
            let icon = if p.icon.trim().is_empty() { p.name.chars().take(2).collect::<String>() } else { p.icon.clone() };
            items.push(Item {
                glyph: Glyph::Text(icon),
                tip: format!("{}\n{}", p.name, p.description),
                action: format!("proc:{}", p.id),
                enabled: true,
            });
        }
        out.push(("ai".to_string(), crate::tl!("Assistant Toolbar").to_string(), items, true));
    }
    for p in app.civil_ai.tools.plugins.active().filter(|p| !p.toolbar.is_empty() && !app.civil_ai.tools.closed_bars.contains(&p.id)) {
        let items: Vec<Item> = p
            .toolbar
            .iter()
            .filter_map(|id| p.procedure(id))
            .map(|proc| {
                let icon = if proc.icon.trim().is_empty() { proc.name.chars().take(2).collect::<String>() } else { proc.icon.clone() };
                Item {
                    glyph: Glyph::Text(icon),
                    tip: format!("{}\n{}", proc.name, proc.description),
                    action: format!("pproc:{}:{}", p.id, proc.id),
                    enabled: true,
                }
            })
            .collect();
        out.push((format!("plugin.{}", p.id), p.name.clone(), items, true));
    }
    out
}

/// Run what a click on one of [`bars`]' buttons stands for.
pub fn run_action(app: &mut CadApp, action: &str) {
    match action {
        "ai:chat" => app.ui.dialog = Some("civil_ai".into()),
        "ai:lib" => app.ui.dialog = Some("ai_procedures".into()),
        "ai:rec" => {
            if app.session.is_recording() {
                stop_recording(app);
            } else {
                start_recording(app);
            }
        }
        _ => {
            let procedure = if let Some(id) = action.strip_prefix("proc:") {
                find(&app.civil_ai.tools, id)
            } else if let Some((plugin, proc)) = action.strip_prefix("pproc:").and_then(|rest| rest.split_once(':')) {
                app.civil_ai.tools.plugins.active().find(|p| p.id == plugin).and_then(|p| p.procedure(proc).cloned())
            } else {
                None
            };
            if let Some(p) = procedure {
                let _ = run_procedure(app, &p);
            }
        }
    }
}

/// Close one of the bars of [`bars`] for this session.
pub fn close_bar(app: &mut CadApp, id: &str) {
    if id == "ai" {
        app.civil_ai.tools.toolbar = false;
    } else if let Some(plugin) = id.strip_prefix("plugin.") {
        app.civil_ai.tools.closed_bars.push(plugin.to_string());
    }
}

/// The procedure library window.
pub fn library_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    if !app.civil_ai.tools.loaded {
        load(&mut app.civil_ai.tools);
    }
    enum Act {
        Run(Procedure),
        Trial(Procedure),
        Improve(Procedure),
        Patch(Procedure),
        Package(Procedure),
        Delete(String),
        Toggle(String),
        Save,
    }
    let mut act: Option<Act> = None;
    let warn = Tokens::get().warn;
    egui::Window::new(crate::tl!("Procedure Library")).id(egui::Id::unique("ai_procedures_dialog")).open(open).default_size(vec2(620.0, 520.0)).show(
        ctx,
        |ui| {
            let st = &mut app.civil_ai.tools;
            if let Some(d) = st.draft.as_mut() {
                draft_ui(ui, d, warn);
                ui.horizontal(|ui| {
                    if ui.button(crate::tl!("Save procedure")).clicked() {
                        act = Some(Act::Save);
                    }
                    if ui.button(crate::tl!("Cancel")).clicked() {
                        st.draft = None;
                    }
                });
                if !st.note.is_empty() {
                    ui.colored_label(warn, st.note.as_str());
                }
                return;
            }
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("New procedure")).clicked() {
                    st.draft =
                        Some(Draft { toolbar: true, steps: "[\n  {\"command\": \"civil.tin\", \"params\": {}}\n]".into(), ..Draft::default() });
                }
                ui.checkbox(&mut st.toolbar, crate::tl!("Show the assistant toolbar"));
            });
            ui.separator();
            let list = all(st);
            egui::ScrollArea::vertical().id_salt("ai_proc_list").max_height(150.0).show(ui, |ui| {
                if list.is_empty() {
                    ui.label(crate::tl!("No procedures yet. Ask the assistant for a sequence of commands and save it from the chat, or record one."));
                }
                for p in &list {
                    let tag = if p.builtin { crate::tl!("built-in") } else { "" };
                    let text = format!("{}  {}  ({} {})", p.name, tag, p.steps.len(), crate::tl!("steps"));
                    if ui.selectable_label(st.selected.as_deref() == Some(p.id.as_str()), text).clicked() {
                        st.selected = Some(p.id.clone());
                        st.patch.clear();
                    }
                }
            });
            ui.separator();
            let Some(p) = st.selected.as_deref().and_then(|id| find(st, id)) else {
                ui.label(crate::tl!("Select a procedure."));
                return;
            };
            ui.label(RichText::new(&p.name).strong());
            if !p.description.is_empty() {
                ui.label(&p.description);
            }
            egui::ScrollArea::vertical().id_salt("ai_proc_steps").max_height(110.0).show(ui, |ui| {
                for (i, s) in p.steps.iter().enumerate() {
                    ui.label(RichText::new(format!("{}. {}", i + 1, s.summary())).monospace());
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button(crate::tl!("Run")).clicked() {
                    act = Some(Act::Run(p.clone()));
                }
                if ui
                    .button(crate::tl!("Test on a copy"))
                    .on_hover_text(crate::tl!("Runs it and undoes it, then prepares a report for the assistant."))
                    .clicked()
                {
                    act = Some(Act::Trial(p.clone()));
                }
                if ui.button(crate::tl!("Ask the assistant to improve it")).clicked() {
                    act = Some(Act::Improve(p.clone()));
                }
                if !p.builtin {
                    let label = if p.toolbar { crate::tl!("Remove from toolbar") } else { crate::tl!("Add to toolbar") };
                    if ui.button(label).clicked() {
                        act = Some(Act::Toggle(p.id.clone()));
                    }
                    if ui
                        .button(crate::tl!("Prepare patch"))
                        .on_hover_text(crate::tl!("Makes the patch that would add this procedure to the application permanently."))
                        .clicked()
                    {
                        act = Some(Act::Patch(p.clone()));
                    }
                    if ui.button(crate::tl!("Context for Claude, Antigravity")).clicked() {
                        act = Some(Act::Package(p.clone()));
                    }
                    if ui.button(crate::tl!("Delete")).clicked() {
                        act = Some(Act::Delete(p.id.clone()));
                    }
                }
            });
            if !st.patch.is_empty() {
                ui.label(crate::tl!("Patch (apply with git apply):"));
                egui::ScrollArea::vertical().id_salt("ai_proc_patch").max_height(100.0).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(st.patch.as_str()).monospace()).wrap());
                });
                if ui.button(crate::tl!("Copy")).clicked() {
                    ui.ctx().copy_text(st.patch.clone());
                }
            }
            if let Some(t) = &st.trial {
                ui.separator();
                ui.label(
                    RichText::new(crate::tf!(
                        "Last trial: {name} (objects {before} → {after}, undone)",
                        name = t.name,
                        before = t.before,
                        after = t.after
                    ))
                    .small(),
                );
                for (s, r) in &t.results {
                    match r {
                        Ok(m) => ui.label(RichText::new(format!("OK  {s}  → {m}")).small().monospace()),
                        Err(e) => ui.label(RichText::new(format!("ERROR  {s}  → {e}")).small().monospace().color(warn)),
                    };
                }
            }
            if !st.note.is_empty() {
                ui.label(RichText::new(st.note.as_str()).small());
            }
        },
    );
    let Some(a) = act else { return };
    match a {
        Act::Run(p) => {
            let results = run_steps(app, &p.steps);
            let st = &mut app.civil_ai.tools;
            st.trial = None;
            st.note = results
                .iter()
                .find_map(|(s, r)| r.as_ref().err().map(|e| format!("{s}: {e}")))
                .unwrap_or_else(|| crate::tf!("{name}: done.", name = p.name));
        }
        Act::Trial(p) => trial(app, &p.name, &p.steps),
        Act::Improve(p) => {
            let st = &mut app.civil_ai.tools;
            st.prefill = Some(format!(
                "Este es un procedimiento mío de TerraCAD-GIS. Revísalo y propón una versión mejorada (más robusta, con parámetros claros y manejo de errores), como bloques ```cadcraft o ```lisp:\n\n```json\n{}\n```",
                procedures::to_jsonl(&p)
            ));
            st.open_chat = true;
            app.ui.dialog = Some("civil_ai".into());
        }
        Act::Patch(p) => {
            let st = &mut app.civil_ai.tools;
            match patch_for(st, &p) {
                Ok(patch) => {
                    st.note = match write_in_settings("ai-patches", &format!("{}.patch", p.id), &patch) {
                        Ok(path) => crate::tf!("Saved to {path}.", path = path.display()),
                        Err(e) => e,
                    };
                    st.patch = patch;
                }
                Err(e) => st.note = e,
            }
        }
        Act::Package(p) => {
            let st = &mut app.civil_ai.tools;
            match source_root(st) {
                None => st.note = crate::tl!("The source code folder was not found. Set it in AI Assistant > Source Code.").to_string(),
                Some(root) => {
                    let text = source::context_package(&root, &st.package_request, Some(&p));
                    st.note = match write_in_settings("ai-context", &format!("{}.md", p.id), &text) {
                        Ok(path) => crate::tf!("Saved to {path}.", path = path.display()),
                        Err(e) => e,
                    };
                    ctx.copy_text(text);
                }
            }
        }
        Act::Delete(id) => {
            let st = &mut app.civil_ai.tools;
            st.lib.remove(&id);
            if st.selected.as_deref() == Some(id.as_str()) {
                st.selected = None;
            }
            persist(st);
        }
        Act::Toggle(id) => {
            let st = &mut app.civil_ai.tools;
            if let Some(p) = st.lib.procedures.iter_mut().find(|p| p.id == id) {
                p.toolbar = !p.toolbar;
                if p.toolbar {
                    st.toolbar = true;
                }
            }
            persist(st);
        }
        Act::Save => save_draft(app),
    }
}

fn draft_ui(ui: &mut egui::Ui, d: &mut Draft, warn: egui::Color32) {
    ui.label(RichText::new(crate::tl!("Procedure")).strong());
    ui.horizontal(|ui| {
        ui.label(crate::tl!("Name"));
        ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(240.0));
        ui.label(crate::tl!("Button"));
        ui.add(egui::TextEdit::singleline(&mut d.icon).desired_width(50.0).char_limit(3));
        ui.checkbox(&mut d.toolbar, crate::tl!("On the toolbar"));
    });
    ui.label(crate::tl!("Description"));
    ui.add(egui::TextEdit::multiline(&mut d.description).desired_rows(2).desired_width(f32::INFINITY));
    ui.label(crate::tl!("Steps (JSON)"));
    ui.add(egui::TextEdit::multiline(&mut d.steps).code_editor().desired_rows(10).desired_width(f32::INFINITY));
    ui.label(RichText::new(crate::tl!("A LISP routine is the step {\"command\": \"lisp\", \"params\": {\"code\": \"...\"}}.")).small().color(warn));
}

fn save_draft(app: &mut CadApp) {
    let Some(d) = app.civil_ai.tools.draft.clone() else { return };
    let steps: Vec<Step> = match serde_json::from_str(&d.steps) {
        Ok(s) => s,
        Err(e) => {
            app.civil_ai.tools.note = crate::tf!("The steps are not valid JSON: {why}", why = e);
            return;
        }
    };
    let st = &mut app.civil_ai.tools;
    let mut id = procedures::slug(&d.name);
    if let Some(old) = &d.editing {
        st.lib.remove(old);
        id = old.clone();
    }
    let p = Procedure {
        id: procedures::unique_id(&id, &st.lib.ids()),
        name: d.name.trim().to_string(),
        description: d.description.trim().to_string(),
        steps,
        icon: d.icon.trim().to_string(),
        toolbar: d.toolbar,
        created: now(),
        notes: d.notes,
        builtin: false,
    };
    let problems = procedures::validate(&p, |c| cadcraft_engine::find_command(c).is_some());
    if !problems.is_empty() {
        st.note = problems.join("\n");
        return;
    }
    match st.lib.add(p.clone()) {
        Ok(id) => {
            st.selected = Some(id);
            st.draft = None;
            if p.toolbar {
                st.toolbar = true;
            }
            persist(st);
        }
        Err(e) => st.note = e,
    }
}

/// The Source Code window: where the code is, whether the assistant gets it, and the package for outside tools.
pub fn source_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let mut package = false;
    egui::Window::new(crate::tl!("Source Code")).id(egui::Id::unique("ai_source_dialog")).open(open).default_size(vec2(540.0, 420.0)).show(ctx, |ui| {
        let st = &mut app.civil_ai.tools;
        ui.label(crate::tl!("The assistant can read the application's source code to propose changes you can copy to Claude, Antigravity or another tool."));
        ui.horizontal(|ui| {
            ui.label(crate::tl!("Folder"));
            ui.add(egui::TextEdit::singleline(&mut st.source_dir).hint_text(crate::tl!("(detected automatically)")).desired_width(340.0));
        });
        match source_root(st) {
            Some(root) => {
                let n = source::list_files(&root).len();
                ui.label(crate::tf!("Found: {path} ({n} files)", path = root.display(), n = n));
            }
            None => {
                ui.colored_label(Tokens::get().warn, crate::tl!("The source code folder was not found. Clone the repository and type its folder here, or set TERRACAD_SOURCE."));
            }
        }
        ui.checkbox(&mut st.use_source, crate::tl!("Send the source code context with each question"));
        ui.label(RichText::new(crate::tl!("Sends the project's rules, a map of the files and the files most related to your question (up to 60 KB). Only source text is sent, never keys or drawings.")).small());
        ui.separator();
        ui.label(crate::tl!("Context package for outside tools"));
        ui.add(egui::TextEdit::multiline(&mut st.package_request).hint_text(crate::tl!("What should the other tool do?")).desired_rows(3).desired_width(f32::INFINITY));
        if ui.button(crate::tl!("Create and copy the package")).clicked() {
            package = true;
        }
        if !st.note.is_empty() {
            ui.label(RichText::new(st.note.as_str()).small());
        }
    });
    if package {
        let st = &mut app.civil_ai.tools;
        match source_root(st) {
            Some(root) => {
                let text = source::context_package(&root, &st.package_request, None);
                st.note = match write_in_settings("ai-context", "contexto-terracad-gis.md", &text) {
                    Ok(path) => crate::tf!("Saved to {path}.", path = path.display()),
                    Err(e) => e,
                };
                ctx.copy_text(text);
            }
            None => st.note = crate::tl!("The source code folder was not found. Set it in AI Assistant > Source Code.").to_string(),
        }
    }
}

/// The Plugin Manager: what is installed, the review of a new plugin, and making one to share.
pub fn plugins_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    ensure_plugins(&mut app.civil_ai.tools);
    if !app.civil_ai.tools.loaded {
        load(&mut app.civil_ai.tools);
    }
    enum Act {
        Pick,
        Install,
        Cancel,
        Remove(String),
        Toggle(String, bool),
        Startup(String, bool),
        Export,
        Reload,
    }
    let mut act: Option<Act> = None;
    let warn = Tokens::get().warn;
    let can_pick = app.services.pick_open.is_some() || app.services.start_pick.is_some();
    egui::Window::new(crate::tl!("Plugin Manager")).id(egui::Id::unique("ai_plugins_dialog")).open(open).default_size(vec2(640.0, 560.0)).show(ctx, |ui| {
        let st = &mut app.civil_ai.tools;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.label(crate::tl!("A plugin adds a menu and toolbar buttons to TerraCAD-GIS without compiling. It is a single .tcgplugin file; you can also drop it on the window."));
            if can_pick && ui.button(crate::tl!("Install plugin...")).clicked() {
                act = Some(Act::Pick);
            }
            if let Some(p) = st.review.clone() {
                ui.separator();
                ui.label(RichText::new(crate::tf!("Review before installing: {name}", name = p.name)).strong());
                ui.label(format!("{} {}  {}", crate::tl!("Version"), p.version, p.author));
                if !p.description.is_empty() {
                    ui.label(&p.description);
                }
                let problems = plugins::validate(&p, command_exists);
                for why in &problems {
                    ui.colored_label(warn, why);
                }
                if let Some(t) = p.menu_title(&crate::menus::reserved_menus()) {
                    ui.label(crate::tf!("Adds the menu {menu} with {n} items.", menu = t, n = p.items.len()));
                }
                ui.label(crate::tf!("{n} procedures, {b} toolbar buttons.", n = p.procedures.len(), b = p.toolbar.len()));
                for pr in &p.procedures {
                    ui.collapsing(format!("{}  ({} {})", pr.name, pr.steps.len(), crate::tl!("steps")), |ui| {
                        for (i, step) in pr.steps.iter().enumerate() {
                            ui.label(RichText::new(format!("{}. {}", i + 1, step.summary())).monospace().small());
                        }
                    });
                }
                for l in &p.lisp {
                    ui.collapsing(format!("LISP: {}", l.name), |ui| {
                        ui.add(egui::Label::new(RichText::new(l.code.as_str()).monospace().small()).wrap());
                    });
                }
                if !p.lisp.is_empty() {
                    ui.colored_label(warn, crate::tl!("This plugin has LISP code. It does not run when installed; you decide afterwards whether it may run at startup."));
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(problems.is_empty(), egui::Button::new(crate::tl!("Install"))).clicked() {
                        act = Some(Act::Install);
                    }
                    if ui.button(crate::tl!("Cancel")).clicked() {
                        act = Some(Act::Cancel);
                    }
                });
            }
            ui.separator();
            ui.label(RichText::new(crate::tl!("Installed plugins")).strong());
            if st.plugins.plugins.is_empty() {
                ui.label(crate::tl!("No plugins installed."));
            }
            let installed = st.plugins.plugins.clone();
            for p in &installed {
                ui.group(|ui| {
                    let mut on = st.plugins.state.enabled(&p.id);
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut on, format!("{} {}", p.name, p.version)).changed() {
                            act = Some(Act::Toggle(p.id.clone(), on));
                        }
                        if ui.button(crate::tl!("Remove")).clicked() {
                            act = Some(Act::Remove(p.id.clone()));
                        }
                    });
                    if !p.description.is_empty() {
                        ui.label(RichText::new(&p.description).small());
                    }
                    ui.label(RichText::new(crate::tf!("Menu: {menu} · {n} procedures", menu = p.menu, n = p.procedures.len())).small());
                    if !p.lisp.is_empty() {
                        let mut allowed = st.plugins.state.may_run_startup(&p.id);
                        if ui.checkbox(&mut allowed, crate::tl!("Allow its LISP code to run at startup")).changed() {
                            act = Some(Act::Startup(p.id.clone(), allowed));
                        }
                    }
                });
            }
            for why in &st.plugins.problems {
                ui.colored_label(warn, why);
            }
            ui.separator();
            ui.label(RichText::new(crate::tl!("Make a plugin from my procedures")).strong());
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Name"));
                ui.add(egui::TextEdit::singleline(&mut st.export_name).desired_width(200.0));
                ui.label(crate::tl!("Menu"));
                ui.add(egui::TextEdit::singleline(&mut st.export_menu).desired_width(160.0));
            });
            let mine: Vec<(String, String)> = st.lib.procedures.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
            if mine.is_empty() {
                ui.label(RichText::new(crate::tl!("Save procedures in the library first.")).small());
            }
            for (id, name) in &mine {
                let mut on = st.export_pick.contains(id);
                if ui.checkbox(&mut on, name.as_str()).changed() {
                    st.export_pick.retain(|x| x != id);
                    if on {
                        st.export_pick.push(id.clone());
                    }
                }
            }
            if ui.add_enabled(!st.export_pick.is_empty() && !st.export_name.trim().is_empty(), egui::Button::new(crate::tl!("Create plugin file"))).clicked() {
                act = Some(Act::Export);
            }
            if ui.button(crate::tl!("Reload")).clicked() {
                act = Some(Act::Reload);
            }
            if !st.note.is_empty() {
                ui.label(RichText::new(st.note.as_str()).small());
            }
        });
    });
    let Some(a) = act else { return };
    match a {
        Act::Pick => {
            let _ = app.pick_then_open(crate::Pick::Open, |app, path| {
                review_file(app, &path);
                Ok(Value::Null)
            });
        }
        Act::Cancel => app.civil_ai.tools.review = None,
        Act::Install => {
            let st = &mut app.civil_ai.tools;
            let Some(p) = st.review.clone() else { return };
            st.note = match plugins_dir() {
                None => crate::tl!("There is no settings folder to save into.").to_string(),
                Some(d) => match plugins::install(&d, &p, command_exists) {
                    Ok(replaced) => {
                        st.review = None;
                        if replaced { crate::tf!("{name} updated.", name = p.name) } else { crate::tf!("{name} installed.", name = p.name) }
                    }
                    Err(e) => e,
                },
            };
            reload_plugins(st);
        }
        Act::Remove(id) => {
            let st = &mut app.civil_ai.tools;
            st.note = match plugins_dir().map(|d| plugins::remove(&d, &id)) {
                Some(Ok(())) => crate::tl!("Removed.").to_string(),
                Some(Err(e)) => e,
                None => crate::tl!("There is no settings folder to save into.").to_string(),
            };
            reload_plugins(st);
        }
        Act::Toggle(id, on) => {
            let st = &mut app.civil_ai.tools;
            st.plugins.state.set_enabled(&id, on);
            if on {
                st.closed_bars.retain(|b| *b != id);
            }
            if let Some(d) = plugins_dir() {
                st.note = st.plugins.save_state(&d).map_or_else(|e| e, |()| String::new());
            }
        }
        Act::Startup(id, on) => {
            let st = &mut app.civil_ai.tools;
            st.plugins.state.set_startup(&id, on);
            if let Some(d) = plugins_dir() {
                st.note =
                    st.plugins.save_state(&d).map_or_else(|e| e, |()| crate::tl!("It will run the next time the application starts.").to_string());
            }
        }
        Act::Reload => reload_plugins(&mut app.civil_ai.tools),
        Act::Export => {
            let st = &mut app.civil_ai.tools;
            let procs: Vec<Procedure> = st.lib.procedures.iter().filter(|p| st.export_pick.contains(&p.id)).cloned().collect();
            let menu = if st.export_menu.trim().is_empty() { st.export_name.clone() } else { st.export_menu.clone() };
            let plugin = plugins::from_procedures(&st.export_name, &st.export_name, &menu, procs);
            let problems = plugins::validate(&plugin, command_exists);
            st.note = if problems.is_empty() {
                match write_in_settings("plugin-export", &format!("{}.{}", plugin.id, plugins::EXTENSION), &plugin.to_text()) {
                    Ok(path) => crate::tf!("Saved to {path}.", path = path.display()),
                    Err(e) => e,
                }
            } else {
                problems.join("\n")
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use cadcraft_engine::Session;
    use serde_json::json;

    use super::*;
    use crate::Services;

    fn circle() -> Step {
        Step { command: "circle".into(), params: json!({"center": [0.0, 0.0], "radius": 1.0}) }
    }

    fn app() -> CadApp {
        CadApp::new(Session::new(), Services::default())
    }

    #[test]
    fn a_trial_run_leaves_the_drawing_as_it_was_and_reports() {
        let mut app = app();
        let bad = Step { command: "no-such-command".into(), params: Value::Null };
        trial(&mut app, "prueba", &[circle(), circle(), bad]);
        assert_eq!(app.session.entity_count(), 0, "the trial is undone");
        let t = app.civil_ai.tools.trial.as_ref().unwrap();
        assert_eq!((t.before, t.after), (0, 2));
        assert!(t.results[2].1.is_err());
        let report = app.civil_ai.tools.prefill.clone().unwrap();
        assert!(report.contains("prueba") && report.contains("ERROR"), "{report}");
    }

    #[test]
    fn a_plugin_menu_item_runs_its_procedure() {
        let mut app = app();
        let proc = Procedure {
            id: "uno".into(),
            name: "Un círculo".into(),
            description: String::new(),
            steps: vec![circle()],
            icon: "C".into(),
            toolbar: true,
            created: 0,
            notes: String::new(),
            builtin: false,
        };
        let plugin = plugins::from_procedures("mi-plugin", "Mi plugin", "Mis cosas", vec![proc]);
        app.civil_ai.tools.plugins.plugins.push(plugin);
        app.civil_ai.tools.plugins_loaded = true;
        let menus = crate::menus::tree(&app);
        let (_, entries) = menus.iter().find(|(m, _)| m == "Mis cosas").expect("the plugin's menu");
        assert!(matches!(&entries[0], crate::menus::Entry::Item { id, .. } if id == "plugin.run:mi-plugin:uno"));
        // Before Window, after the application's own menus.
        let pos = |name: &str| menus.iter().position(|(m, _)| m == name).unwrap();
        assert!(pos("Mis cosas") > pos("AI Assistant") && pos("Mis cosas") < pos("Window"));
        app.start("plugin.run:mi-plugin:uno");
        assert_eq!(app.session.entity_count(), 1);
        // Switched off: no menu, and the item no longer runs.
        app.civil_ai.tools.plugins.state.set_enabled("mi-plugin", false);
        assert!(!crate::menus::tree(&app).iter().any(|(m, _)| m == "Mis cosas"));
        assert!(run_plugin_item(&mut app, "plugin.run:mi-plugin:uno").unwrap().is_err());
        assert_eq!(app.session.entity_count(), 1);
    }

    #[test]
    fn startup_lisp_runs_only_when_allowed() {
        let mut app = app();
        let mut plugin = plugins::from_procedures("lisp-plug", "Lisp", "", vec![]);
        plugin.lisp.push(plugins::LispFile { name: "a.lsp".into(), code: "(command \"CIRCLE\" \"0,0\" \"1\")".into() });
        app.civil_ai.tools.plugins.plugins.push(plugin);
        app.civil_ai.tools.plugins_loaded = true;
        startup(&mut app);
        assert_eq!(app.session.entity_count(), 0, "not allowed: nothing runs");
        app.civil_ai.tools.startup_done = false;
        app.civil_ai.tools.plugins.state.set_startup("lisp-plug", true);
        startup(&mut app);
        assert_eq!(app.session.entity_count(), 1);
    }
}
