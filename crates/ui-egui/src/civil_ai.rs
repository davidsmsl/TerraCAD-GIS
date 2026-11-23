//! The AI assistant window of TerraCAD-GIS (own top-level menu): a chat with Anthropic (Claude), Google
//! (Gemini), DeepSeek or a local engine (LM Studio, Núcleo IA, Ollama) that can propose commands
//! and code improvements. The person picks the model (listed live from the service), the
//! reasoning level, and can attach files.
//!
//! The person stays in charge. A reply may carry commands (```cadcraft blocks); each is listed
//! with a Run button and only runs when pressed, through the same registered commands the menus
//! and the automation interface use. Code and improvement proposals are shown to copy or save.
//! The API key is the person's own: typed here, or read from `ANTHROPIC_API_KEY`,
//! `GEMINI_API_KEY` or `DEEPSEEK_API_KEY`. Each service keeps its own key and model. It is saved to disk only when the person ticks the box, and never logged.

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

use cadcraft_civil::ai::{self, AiError, Attachment, Message, Proposal, Provider, Reasoning, Role, Settings};
use cadcraft_civil::procedures::{self, Step};
use egui::{RichText, vec2};
use serde_json::Value;

use crate::CadApp;
use crate::theme::Tokens;

/// One line of the conversation as shown.
struct Entry {
    role: Role,
    text: String,
    proposals: Vec<Proposal>,
    /// What each proposal answered when it was run, in the same order.
    results: Vec<Option<String>>,
    code: Vec<ai::CodeBlock>,
    failed: bool,
    /// Files sent with this message (kept to resend with it, and to show their names).
    files: Vec<Attachment>,
    /// The commands and LISP routines of an assistant reply, in order ([`procedures::steps_from_reply`]).
    steps: Vec<Step>,
}

/// What the buttons under a reply do with its steps.
enum StepsAct {
    Run,
    Trial,
    Save,
}

/// The window's state, kept in [`CadApp`].
#[derive(Default)]
pub struct State {
    loaded: bool,
    settings: Settings,
    remember_key: bool,
    show_settings: bool,
    chat: Vec<Entry>,
    input: String,
    waiting: Option<Receiver<Result<ai::Answer, AiError>>>,
    note: String,
    /// Key and model typed for each service (by id), so switching services keeps them.
    keys: BTreeMap<String, String>,
    models: BTreeMap<String, String>,
    /// Models each service reported the last time the person asked (by service id).
    listed: BTreeMap<String, Vec<String>>,
    /// A model list being fetched.
    refreshing: Option<Receiver<Result<Vec<String>, AiError>>>,
    /// Files waiting to go with the next message.
    attachments: Vec<Attachment>,
    /// The procedure library, toolbar, trial runs, recording and source code ([`crate::ai_tools`]).
    pub tools: crate::ai_tools::State,
    /// Changes to the application's own source, the restart reminder and the GitHub connection ([`crate::ai_self`]).
    pub selfmod: crate::ai_self::State,
    /// A test or a save to GitHub asked for from a menu; the window starts it on its next frame.
    want_test: bool,
    want_github_save: bool,
}

impl State {
    /// Ask for the connection test (menu command).
    pub fn request_test(&mut self) {
        self.want_test = true;
    }

    /// Ask for the save to GitHub (menu command).
    pub fn request_github_save(&mut self) {
        self.want_github_save = true;
    }
}

/// What is kept between runs.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Saved {
    provider: String,
    /// Model chosen for each service, by id.
    #[serde(default)]
    models: BTreeMap<String, String>,
    extra: String,
    /// Where the local engine listens.
    #[serde(default)]
    local_url: String,
    /// Reasoning level id (`auto`, `low`, `medium`, `high`, `max`).
    #[serde(default)]
    reasoning: String,
    /// Where the application's source code is ("" = detect) and whether to send it with questions.
    #[serde(default)]
    source_dir: String,
    #[serde(default)]
    use_source: bool,
    #[serde(default)]
    toolbar: bool,
    #[serde(default)]
    github_repo: String,
    #[serde(default)]
    github_branch: String,
    #[serde(default)]
    github_auto: bool,
    /// The GitHub token, kept only when the person ticked the box to remember keys.
    #[serde(default)]
    github_token: String,
    /// Keys the person asked to remember, by service id.
    #[serde(default)]
    keys: BTreeMap<String, String>,
    // Files written before several services were kept: one model and one key for `provider`.
    #[serde(default, skip_serializing)]
    model: String,
    #[serde(default, skip_serializing)]
    key: Option<String>,
}

/// Put the active service's model and key back from what is kept for it, or from the environment.
fn activate(st: &mut State) {
    let id = st.settings.provider.id().to_string();
    st.settings.model = st.models.get(&id).cloned().unwrap_or_default();
    st.settings.api_key =
        st.keys.get(&id).cloned().filter(|k| !k.is_empty()).unwrap_or_else(|| std::env::var(st.settings.provider.key_variable()).unwrap_or_default());
}

/// Keep what is typed for the active service.
fn stash(st: &mut State) {
    let id = st.settings.provider.id().to_string();
    st.models.insert(id.clone(), st.settings.model.clone());
    st.keys.insert(id, st.settings.api_key.clone());
}

#[cfg(not(target_arch = "wasm32"))]
fn file() -> Option<std::path::PathBuf> {
    cadcraft_engine::settings_dir::current().map(|d| d.join("civil-ai.json"))
}

#[cfg(not(target_arch = "wasm32"))]
fn load(st: &mut State) {
    st.loaded = true;
    st.settings.provider = Provider::Anthropic;
    let saved: Saved = file().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if let Some(p) = Provider::from_id(&saved.provider) {
        st.settings.provider = p;
    }
    st.settings.extra_instructions = saved.extra;
    st.settings.reasoning = Reasoning::from_id(&saved.reasoning).unwrap_or_default();
    st.tools.source_dir = saved.source_dir;
    st.tools.use_source = saved.use_source;
    st.tools.toolbar = saved.toolbar;
    st.selfmod.gh.repo = saved.github_repo;
    st.selfmod.gh.branch = saved.github_branch;
    st.selfmod.auto_save = saved.github_auto;
    st.selfmod.gh.token = if saved.github_token.is_empty() { crate::ai_self::token_from_env() } else { saved.github_token.clone() };
    st.settings.base_url = if saved.local_url.trim().is_empty() { ai::DEFAULT_LOCAL_URL.to_string() } else { saved.local_url };
    st.models = saved.models;
    st.keys = saved.keys;
    if let (Some(k), false) = (saved.key, st.settings.provider.id().is_empty()) {
        st.keys.entry(st.settings.provider.id().to_string()).or_insert(k);
    }
    if !saved.model.is_empty() {
        st.models.entry(st.settings.provider.id().to_string()).or_insert(saved.model);
    }
    st.remember_key = st.keys.values().any(|k| !k.is_empty()) || !saved.github_token.is_empty();
    activate(st);
    crate::ai_self::ensure_loaded(&mut st.selfmod);
    load_chat(st);
}

#[cfg(target_arch = "wasm32")]
fn load(st: &mut State) {
    st.loaded = true;
}

#[cfg(not(target_arch = "wasm32"))]
fn save(st: &State) -> std::io::Result<()> {
    use std::io::Write;
    let Some(path) = file() else { return Ok(()) };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let (mut models, mut keys) = (st.models.clone(), st.keys.clone());
    models.insert(st.settings.provider.id().into(), st.settings.model.clone());
    keys.insert(st.settings.provider.id().into(), st.settings.api_key.clone());
    // Only keys the person typed are kept, and only when they ticked the box.
    keys.retain(|id, k| {
        st.remember_key && !k.is_empty() && Provider::from_id(id).is_none_or(|p| std::env::var(p.key_variable()).ok().as_deref() != Some(k.as_str()))
    });
    let saved = Saved {
        provider: st.settings.provider.id().into(),
        models,
        extra: st.settings.extra_instructions.clone(),
        local_url: st.settings.base_url.clone(),
        reasoning: st.settings.reasoning.id().into(),
        source_dir: st.tools.source_dir.clone(),
        use_source: st.tools.use_source,
        toolbar: st.tools.toolbar,
        github_repo: st.selfmod.gh.repo.clone(),
        github_branch: st.selfmod.gh.branch.clone(),
        github_auto: st.selfmod.auto_save,
        github_token: if st.remember_key && std::env::var("GITHUB_TOKEN").ok().as_deref() != Some(st.selfmod.gh.token.as_str()) {
            st.selfmod.gh.token.clone()
        } else {
            String::new()
        },
        keys,
        model: String::new(),
        key: None,
    };
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    // A key on disk is readable by its owner only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path)?;
    f.write_all(&serde_json::to_vec_pretty(&saved).unwrap_or_default())
}

#[cfg(target_arch = "wasm32")]
fn save(_: &State) -> std::io::Result<()> {
    Ok(())
}

/// One line of the conversation as kept between sessions.
#[derive(serde::Serialize, serde::Deserialize)]
struct KeptLine {
    /// `user` or `assistant`.
    role: String,
    text: String,
}

/// The most lines kept between sessions.
const KEEP_LINES: usize = 60;

#[cfg(not(target_arch = "wasm32"))]
fn chat_file() -> Option<std::path::PathBuf> {
    cadcraft_engine::settings_dir::current().map(|d| d.join("civil-ai-chat.json"))
}

/// Put the conversation of the last session back, so a restart (or the next day) continues it.
#[cfg(not(target_arch = "wasm32"))]
fn load_chat(st: &mut State) {
    let kept: Vec<KeptLine> = chat_file().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    for l in kept {
        let role = if l.role == "user" { Role::User } else { Role::Assistant };
        st.chat.push(if role == Role::User { entry_user(l.text, Vec::new()) } else { entry_reply(l.text) });
    }
}

#[cfg(target_arch = "wasm32")]
fn load_chat(_: &mut State) {}

/// Keep the conversation for the next session (text only; owner-readable).
#[cfg(not(target_arch = "wasm32"))]
fn save_chat(st: &State) {
    use std::io::Write;
    let Some(path) = chat_file() else { return };
    let kept: Vec<KeptLine> = st
        .chat
        .iter()
        .filter(|e| !e.failed)
        .map(|e| KeptLine { role: if e.role == Role::User { "user".into() } else { "assistant".into() }, text: e.text.clone() })
        .collect();
    let kept = &kept[kept.len().saturating_sub(KEEP_LINES)..];
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    if let Ok(mut f) = o.open(path) {
        let _ = f.write_all(&serde_json::to_vec(kept).unwrap_or_default());
    }
}

#[cfg(target_arch = "wasm32")]
fn save_chat(_: &State) {}

fn entry_user(text: String, files: Vec<Attachment>) -> Entry {
    Entry { role: Role::User, text, proposals: Vec::new(), results: Vec::new(), code: Vec::new(), failed: false, files, steps: Vec::new() }
}

/// An assistant line with its commands, code and steps read out of `text`.
fn entry_reply(text: String) -> Entry {
    let proposals = ai::proposals(&text);
    let code = ai::code_blocks(&text).into_iter().filter(|b| b.lang != "cadcraft").collect();
    Entry {
        role: Role::Assistant,
        results: vec![None; proposals.len()],
        proposals,
        code,
        steps: procedures::steps_from_reply(&text),
        text,
        failed: false,
        files: Vec::new(),
    }
}

/// The messages to send: what was said, as text. Files go only with the latest message that
/// carried them; earlier ones are mentioned by name so the conversation stays light.
fn history(chat: &[Entry]) -> Vec<Message> {
    let last_with_files = chat.iter().rposition(|e| !e.failed && !e.files.is_empty());
    chat.iter()
        .enumerate()
        .filter(|(_, e)| !e.failed)
        .map(|(i, e)| {
            if e.files.is_empty() {
                Message::new(e.role, e.text.clone())
            } else if Some(i) == last_with_files {
                Message { role: e.role, text: e.text.clone(), attachments: e.files.clone() }
            } else {
                let names: Vec<&str> = e.files.iter().map(|f| f.name.as_str()).collect();
                Message::new(e.role, format!("{}\n[Archivos adjuntados antes: {}]", e.text, names.join(", ")))
            }
        })
        .collect()
}

fn send(app: &mut CadApp, ctx: &egui::Context) {
    let st = &mut app.civil_ai;
    let text = st.input.trim().to_string();
    if text.is_empty() || st.waiting.is_some() {
        return;
    }
    let files = std::mem::take(&mut st.attachments);
    st.chat.push(entry_user(text, files));
    save_chat(st);
    st.input.clear();
    st.note.clear();
    let (mut settings, msgs) = (st.settings.clone(), history(&st.chat));
    // The source code, when the person asked for it, goes with the instructions for this question.
    let context = crate::ai_tools::context_for(&st.tools, &st.chat.last().map(|e| e.text.clone()).unwrap_or_default());
    settings.extra_instructions.push_str(&context);
    let (tx, rx) = channel();
    st.waiting = Some(rx);
    let ctx = ctx.clone();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let spawned = std::thread::Builder::new().name("civil-ai".into()).spawn(move || {
            let _ = tx.send(ai::chat_answer(&settings, &msgs));
            ctx.request_repaint();
        });
        if spawned.is_err() {
            st.waiting = None;
            st.note = crate::tl!("Could not start the request.").to_string();
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (tx.send(ai::chat_answer(&settings, &msgs)), ctx);
    }
}

/// Ask the service for the models it offers now (in the background).
fn refresh_models(st: &mut State, ctx: &egui::Context) {
    if st.refreshing.is_some() {
        return;
    }
    let settings = st.settings.clone();
    let (tx, rx) = channel();
    st.refreshing = Some(rx);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("civil-ai-models".into()).spawn(move || {
            let _ = tx.send(ai::list_models(&settings));
            ctx.request_repaint();
        });
        if spawned.is_err() {
            st.refreshing = None;
            st.note = crate::tl!("Could not start the request.").to_string();
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (tx.send(ai::list_models(&settings)), ctx);
    }
}

/// Take the model list if it has come.
fn poll_models(st: &mut State) {
    let Some(rx) = &st.refreshing else { return };
    let answer = match rx.try_recv() {
        Ok(a) => a,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Err(AiError("the request ended without an answer".into())),
    };
    st.refreshing = None;
    match answer {
        Ok(names) => {
            if st.settings.model.trim().is_empty() {
                st.settings.model = names.first().cloned().unwrap_or_default();
            }
            st.note = crate::tf!("{n} models found.", n = names.len());
            st.listed.insert(st.settings.provider.id().to_string(), names);
        }
        Err(e) => st.note = e.0,
    }
}

/// Attach the file at `path` to the next message.
fn attach(st: &mut State, path: &str) {
    if st.attachments.len() >= ai::MAX_ATTACHMENTS {
        st.note = crate::tf!("At most {n} files per message.", n = ai::MAX_ATTACHMENTS);
        return;
    }
    let name = std::path::Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().to_string());
    match std::fs::read(path).map_err(|e| AiError(e.to_string())).and_then(|b| Attachment::from_file(&name, b)) {
        Ok(a) => {
            st.note.clear();
            st.attachments.push(a);
        }
        Err(e) => st.note = e.0,
    }
}

/// Take the reply if it has come.
fn poll(st: &mut State) {
    poll_models(st);
    crate::ai_self::poll(&mut st.selfmod);
    let Some(rx) = &st.waiting else { return };
    let answer = match rx.try_recv() {
        Ok(a) => a,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Err(AiError("the request ended without an answer".into())),
    };
    st.waiting = None;
    match answer {
        Ok(a) => {
            if a.truncated {
                st.note = crate::tl!("The reply is still incomplete after continuing it; write Continue to get the rest.").to_string();
            }
            st.chat.push(entry_reply(a.text));
            save_chat(st);
        }
        Err(e) => st.chat.push(Entry {
            role: Role::Assistant,
            text: e.0,
            proposals: Vec::new(),
            results: Vec::new(),
            code: Vec::new(),
            failed: true,
            files: Vec::new(),
            steps: Vec::new(),
        }),
    }
}

/// Run one proposed command through the engine and say what it answered.
fn run(app: &mut CadApp, p: &Proposal) -> String {
    if cadcraft_engine::find_command(&p.command).is_none() {
        return crate::tf!("Unknown command {name}.", name = p.command);
    }
    match app.session.execute(&p.command, &p.params) {
        Ok(v) => v.get("message").and_then(Value::as_str).map_or_else(|| crate::tl!("Done.").to_string(), str::to_string),
        Err(e) => e.to_string(),
    }
}

/// Save a proposal to the settings folder and say where it went.
#[cfg(not(target_arch = "wasm32"))]
fn save_proposal(text: &str) -> String {
    let Some(dir) = cadcraft_engine::settings_dir::current().map(|d| d.join("ai-proposals")) else {
        return crate::tl!("There is no settings folder to save into.").to_string();
    };
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let path = dir.join(format!("proposal-{secs}.md"));
    match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, text)) {
        Ok(()) => crate::tf!("Saved to {path}.", path = path.display()),
        Err(e) => e.to_string(),
    }
}

#[cfg(target_arch = "wasm32")]
fn save_proposal(_: &str) -> String {
    crate::tl!("Saving is not available in the web version; copy the text instead.").to_string()
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    if !app.civil_ai.loaded {
        load(&mut app.civil_ai);
    }
    poll(&mut app.civil_ai);
    // A question prepared by a recording, a trial run or the library waits in the chat box.
    if let Some(text) = app.civil_ai.tools.prefill.take() {
        app.civil_ai.input = text;
    }
    if app.civil_ai.waiting.is_some() {
        ctx.request_repaint_after(Duration::from_millis(250));
    }
    let warn = Tokens::get().warn;
    let mut to_run: Option<(usize, usize)> = None;
    let mut to_save: Option<String> = None;
    let mut do_send = false;
    let mut do_pick = false;
    let mut steps_act: Option<(usize, StepsAct)> = None;
    let mut test_ai = false;
    let mut test_github = false;
    let mut save_github = false;
    let mut to_review: Option<String> = None;
    let mut self_act: Option<crate::ai_self::Act> = None;
    let mut confirm_answer: Option<bool> = None;
    let mut other_dialog: Option<&str> = None;
    let (recording, recorded) = (app.session.is_recording(), app.session.recorded_lines());
    let can_pick = app.services.pick_open.is_some() || app.services.start_pick.is_some();
    egui::Window::new(crate::tl!("AI Assistant")).id(egui::Id::unique("civil_ai_dialog")).open(open).default_size(vec2(560.0, 620.0)).show(
        ctx,
        |ui| {
            let st = &mut app.civil_ai;
            ui.horizontal(|ui| {
                ui.label(st.settings.provider.label());
                if ui.button(crate::tl!("Settings")).clicked() {
                    st.show_settings = !st.show_settings;
                }
                if ui
                    .add_enabled(!st.selfmod.busy_ai_test(), egui::Button::new(crate::tl!("Test connection")))
                    .on_hover_text(crate::tl!("Checks the address, the key and the model with a tiny request."))
                    .clicked()
                {
                    test_ai = true;
                }
                if ui.button(crate::tl!("New conversation")).clicked() && st.waiting.is_none() {
                    st.chat.clear();
                    st.note.clear();
                    save_chat(st);
                }
                if ui.button(crate::tl!("Procedure Library")).clicked() {
                    other_dialog = Some("ai_procedures");
                }
                if ui.button(crate::tl!("Source Code")).clicked() {
                    other_dialog = Some("ai_source");
                }
                if recording {
                    if ui.button(RichText::new(crate::tf!("■ Stop recording ({n})", n = recorded)).color(warn)).clicked() {
                        other_dialog = Some("stop_recording");
                    }
                } else if ui
                    .button(crate::tl!("● Record"))
                    .on_hover_text(crate::tl!("Records what you do so the assistant can turn it into a procedure."))
                    .clicked()
                {
                    other_dialog = Some("start_recording");
                }
            });
            if st.show_settings || st.settings.provider.needs_key() && st.settings.api_key.trim().is_empty() {
                let (a, b, c) = settings_ui(ui, st, warn);
                test_ai |= a;
                test_github |= b;
                save_github |= c;
            }
            if st.selfmod.busy_ai_test() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(crate::tl!("Testing the connection..."));
                });
            }
            if let Some(r) = &st.selfmod.ai_report {
                crate::ai_self::report_ui(ui, r);
            }
            self_act = crate::ai_self::banner(ui, &st.selfmod);
            confirm_answer = crate::ai_self::confirm_ui(ui, &mut st.selfmod);
            ui.separator();
            egui::ScrollArea::vertical().max_height(ui.available_height() - 110.0).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                if st.chat.is_empty() {
                    ui.label(crate::tl!(
                        "Ask for help with surveys, contours, profiles or the app itself. Proposed commands run only when you press Run."
                    ));
                }
                for (ei, e) in st.chat.iter().enumerate() {
                    let who = if e.role == Role::User { crate::tl!("You") } else { crate::tl!("Assistant") };
                    ui.label(RichText::new(who).strong());
                    if !e.files.is_empty() {
                        let names: Vec<&str> = e.files.iter().map(|f| f.name.as_str()).collect();
                        ui.label(RichText::new(format!("📎 {}", names.join(", "))).small());
                    }
                    if e.failed {
                        ui.colored_label(warn, &e.text);
                    } else {
                        ui.add(egui::Label::new(e.text.as_str()).wrap());
                    }
                    for (pi, p) in e.proposals.iter().enumerate() {
                        ui.group(|ui| {
                            let shown = serde_json::to_string(&p.params).unwrap_or_default();
                            ui.label(RichText::new(format!("{} {}", p.command, shown)).monospace());
                            match e.results.get(pi).and_then(Option::as_ref) {
                                Some(r) => {
                                    ui.label(r.as_str());
                                }
                                None => {
                                    if ui.button(crate::tl!("Run")).clicked() {
                                        to_run = Some((ei, pi));
                                    }
                                }
                            }
                        });
                    }
                    if e.role == Role::Assistant && !e.failed {
                        let n = e.steps.len();
                        if n > 0 {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(crate::tf!("{n} steps", n = n)).small());
                                if ui.button(crate::tl!("Run all")).clicked() {
                                    steps_act = Some((ei, StepsAct::Run));
                                }
                                if ui
                                    .button(crate::tl!("Test on a copy"))
                                    .on_hover_text(crate::tl!("Runs it and undoes it, then prepares a report for the assistant."))
                                    .clicked()
                                {
                                    steps_act = Some((ei, StepsAct::Trial));
                                }
                                if ui.button(crate::tl!("Save as procedure...")).clicked() {
                                    steps_act = Some((ei, StepsAct::Save));
                                }
                            });
                        }
                    }
                    for b in &e.code {
                        ui.collapsing(if b.lang.is_empty() { crate::tl!("Code").to_string() } else { b.lang.clone() }, |ui| {
                            ui.add(egui::Label::new(RichText::new(b.text.as_str()).monospace()).wrap());
                            ui.horizontal(|ui| {
                                if ui.button(crate::tl!("Copy")).clicked() {
                                    ui.ctx().copy_text(b.text.clone());
                                }
                                if ui.button(crate::tl!("Save proposal")).clicked() {
                                    to_save = Some(b.text.clone());
                                }
                                if cadcraft_civil::selfmod::looks_like_diff(&b.text)
                                    && ui
                                        .button(crate::tl!("Apply to the source code..."))
                                        .on_hover_text(crate::tl!("Shows what it changes first; it is saved as its own git commit."))
                                        .clicked()
                                {
                                    to_review = Some(b.text.clone());
                                }
                            });
                        });
                    }
                    ui.add_space(6.0);
                }
                if st.waiting.is_some() {
                    ui.label(crate::tl!("Waiting for the assistant..."));
                }
            });
            if !st.note.is_empty() {
                ui.label(RichText::new(st.note.as_str()).small());
            }
            ui.horizontal_wrapped(|ui| {
                if can_pick && ui.add_enabled(st.waiting.is_none(), egui::Button::new(crate::tl!("Attach file..."))).clicked() {
                    do_pick = true;
                }
                let mut drop: Option<usize> = None;
                for (i, a) in st.attachments.iter().enumerate() {
                    if ui.small_button(format!("{}  ✕", a.name)).on_hover_text(crate::tl!("Remove")).clicked() {
                        drop = Some(i);
                    }
                }
                if let Some(i) = drop {
                    st.attachments.remove(i);
                }
            });
            ui.horizontal(|ui| {
                let r = ui.add(
                    egui::TextEdit::multiline(&mut st.input)
                        .desired_rows(3)
                        .desired_width(ui.available_width() - 90.0)
                        .hint_text(crate::tl!("Write your question")),
                );
                let enter = r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
                let can = st.waiting.is_none()
                    && !st.input.trim().is_empty()
                    && (!st.settings.provider.needs_key() || !st.settings.api_key.trim().is_empty());
                if (ui.add_enabled(can, egui::Button::new(crate::tl!("Send"))).clicked() || enter && can) && can {
                    do_send = true;
                }
            });
        },
    );
    let root = crate::ai_tools::source_root(&app.civil_ai.tools);
    let ctx2 = ctx.clone();
    test_ai |= std::mem::take(&mut app.civil_ai.want_test);
    save_github |= std::mem::take(&mut app.civil_ai.want_github_save);
    if test_ai {
        let settings = app.civil_ai.settings.clone();
        crate::ai_self::start_ai_test(&mut app.civil_ai.selfmod, settings, &ctx2);
    }
    if test_github {
        crate::ai_self::start_github_test(&mut app.civil_ai.selfmod, &ctx2);
    }
    if save_github {
        let message = crate::tl!("Progress saved from the TerraCAD-GIS assistant").to_string();
        crate::ai_self::start_push(&mut app.civil_ai.selfmod, root.clone(), message, &ctx2);
    }
    if let Some(patch) = to_review {
        crate::ai_self::review(&mut app.civil_ai.selfmod, root.as_deref(), &patch);
    }
    match confirm_answer {
        Some(true) => {
            app.civil_ai.note = match crate::ai_self::apply_confirmed(&mut app.civil_ai.selfmod, root.as_deref(), &ctx2) {
                Ok(line) | Err(line) => line,
            };
        }
        Some(false) => app.civil_ai.selfmod.confirm = None,
        None => {}
    }
    if let Some(act) = self_act {
        crate::ai_self::run_act(app, act, root.clone(), &ctx2);
    }
    if app.civil_ai.selfmod.busy_ai_test() || app.civil_ai.selfmod.busy_github() {
        ctx.request_repaint_after(Duration::from_millis(250));
    }
    if let Some((ei, pi)) = to_run {
        let p = app.civil_ai.chat.get(ei).and_then(|e| e.proposals.get(pi)).cloned();
        if let Some(p) = p {
            let result = run(app, &p);
            if let Some(slot) = app.civil_ai.chat.get_mut(ei).and_then(|e| e.results.get_mut(pi)) {
                *slot = Some(result);
            }
        }
    }
    if let Some(t) = to_save {
        app.civil_ai.note = save_proposal(&t);
    }
    match other_dialog {
        Some("start_recording") => crate::ai_tools::start_recording(app),
        Some("stop_recording") => crate::ai_tools::stop_recording(app),
        Some(d) => app.ui.dialog = Some(d.to_string()),
        None => {}
    }
    if let Some((ei, act)) = steps_act {
        let steps = app.civil_ai.chat.get(ei).map(|e| e.steps.clone()).unwrap_or_default();
        match act {
            StepsAct::Run => {
                let results = crate::ai_tools::run_steps(app, &steps);
                app.civil_ai.note = match results.iter().find_map(|(s, r)| r.as_ref().err().map(|e| format!("{s}: {e}"))) {
                    Some(e) => e,
                    None => crate::tf!("{n} steps ran.", n = results.len()),
                };
            }
            StepsAct::Trial => crate::ai_tools::trial(app, crate::tl!("Assistant proposal"), &steps),
            StepsAct::Save => crate::ai_tools::draft_from_steps(app, &steps, crate::tl!("New procedure")),
        }
    }
    if do_pick {
        // Not bound to a drawing: the file goes to the assistant, not into the drawing.
        let _ = app.pick_then_open(crate::Pick::Open, |app, path| {
            attach(&mut app.civil_ai, &path);
            Ok(Value::Null)
        });
    }
    if do_send {
        send(app, ctx);
    }
}

fn reasoning_label(r: Reasoning) -> String {
    match r {
        Reasoning::Auto => crate::tl!("Automatic"),
        Reasoning::Low => crate::tl!("Low"),
        Reasoning::Medium => crate::tl!("Medium"),
        Reasoning::High => crate::tl!("High"),
        Reasoning::Max => crate::tl!("Maximum"),
    }
    .to_string()
}

/// The settings panel. Returns which buttons were pressed: test the assistant, test GitHub, save to GitHub.
fn settings_ui(ui: &mut egui::Ui, st: &mut State, warn: egui::Color32) -> (bool, bool, bool) {
    let mut pressed = (false, false, false);
    ui.group(|ui| {
        let old = st.settings.provider;
        ui.horizontal(|ui| {
            ui.label(crate::tl!("Service"));
            craft_ui::ComboBox::from_id_salt("civil_ai_provider").selected_text(st.settings.provider.label()).show_ui(ui, |ui| {
                for p in Provider::ALL {
                    ui.selectable_value(&mut st.settings.provider, p, p.label());
                }
            });
        });
        if st.settings.provider != old {
            let new = st.settings.provider;
            st.settings.provider = old;
            stash(st);
            st.settings.provider = new;
            activate(st);
        }
        if st.settings.provider == Provider::Local {
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Address"));
                ui.add(egui::TextEdit::singleline(&mut st.settings.base_url).hint_text(ai::DEFAULT_LOCAL_URL).desired_width(260.0));
                craft_ui::ComboBox::from_id_salt("civil_ai_local_preset").selected_text(crate::tl!("Presets")).show_ui(ui, |ui| {
                    for (name, url) in ai::LOCAL_PRESETS {
                        if ui.selectable_label(st.settings.base_url == url, format!("{name}  {url}")).clicked() {
                            st.settings.base_url = url.to_string();
                        }
                    }
                });
            });
            ui.label(
                RichText::new(crate::tl!(
                    "Start the server in LM Studio, Núcleo IA or Ollama first; paste the address of its local API (OpenAI compatible)."
                ))
                .small(),
            );
        }
        ui.horizontal(|ui| {
            ui.label(crate::tl!("Model"));
            ui.add(egui::TextEdit::singleline(&mut st.settings.model).hint_text(st.settings.provider.default_model()).desired_width(220.0));
            craft_ui::ComboBox::from_id_salt("civil_ai_model").selected_text(crate::tl!("Available models")).show_ui(ui, |ui| {
                // What the service reported, then the built-in suggestions it did not repeat.
                let mut names: Vec<String> = st.listed.get(st.settings.provider.id()).cloned().unwrap_or_default();
                for m in st.settings.provider.suggested_models() {
                    if !names.iter().any(|n| n == m) {
                        names.push((*m).to_string());
                    }
                }
                for m in names {
                    if ui.selectable_label(st.settings.model == m, m.as_str()).clicked() {
                        st.settings.model = m;
                    }
                }
            });
            let can = !st.settings.provider.needs_key() || !st.settings.api_key.trim().is_empty();
            if ui.add_enabled(can && st.refreshing.is_none(), egui::Button::new(crate::tl!("Refresh models"))).clicked() {
                refresh_models(st, ui.ctx());
            }
        });
        if st.settings.provider.supports_reasoning() {
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Reasoning"));
                craft_ui::ComboBox::from_id_salt("civil_ai_reasoning").selected_text(reasoning_label(st.settings.reasoning)).show_ui(ui, |ui| {
                    for r in Reasoning::ALL {
                        ui.selectable_value(&mut st.settings.reasoning, r, reasoning_label(r));
                    }
                });
            });
        } else {
            ui.label(RichText::new(crate::tl!("This service decides how much to think by itself.")).small());
        }
        ui.horizontal(|ui| {
            ui.label(if st.settings.provider.needs_key() { crate::tl!("API key") } else { crate::tl!("API key (optional)") });
            ui.add(egui::TextEdit::singleline(&mut st.settings.api_key).password(true).desired_width(280.0));
        });
        ui.label(RichText::new(crate::tf!("Or set the {name} environment variable.", name = st.settings.provider.key_variable())).small());
        if !st.settings.provider.key_page().is_empty() {
            ui.hyperlink_to(crate::tl!("Get an API key"), st.settings.provider.key_page());
        }
        ui.checkbox(&mut st.remember_key, crate::tl!("Remember the keys on this computer (stored unencrypted, readable only by you)"));
        ui.label(crate::tl!("Extra instructions"));
        ui.add(egui::TextEdit::multiline(&mut st.settings.extra_instructions).desired_rows(2).desired_width(f32::INFINITY));
        if st.settings.provider.needs_key() && st.settings.api_key.trim().is_empty() {
            ui.colored_label(warn, crate::tl!("Enter an API key to use the assistant."));
        }
        let (gt, gs) = crate::ai_self::github_ui(ui, &mut st.selfmod);
        pressed.1 = gt;
        pressed.2 = gs;
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button(crate::tl!("Save settings")).clicked() {
                st.note = match save(st) {
                    Ok(()) => crate::tl!("Settings saved.").to_string(),
                    Err(e) => e.to_string(),
                };
            }
            if ui.add_enabled(!st.selfmod.busy_ai_test(), egui::Button::new(crate::tl!("Test connection"))).clicked() {
                pressed.0 = true;
            }
        });
    });
    pressed
}
