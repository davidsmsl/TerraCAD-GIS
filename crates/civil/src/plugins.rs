//! Plugins ("complementos"): a single file that adds a menu, toolbar buttons and, if the person
//! allows it, LISP commands to TerraCAD-GIS without compiling anything.
//!
//! A plugin is a JSON file (`*.tcgplugin`) holding procedures ([`crate::procedures`]), the menu
//! items that run them, the procedures shown as toolbar buttons and optional LISP code loaded
//! at startup. It is the installer: the person reviews it in the Plugin Manager and installs it,
//! which copies the file into the settings folder's `plugins` folder. Nothing in a plugin runs
//! when it is installed. A menu item or button runs its procedure when pressed, through the
//! registered commands. Startup LISP runs only for a plugin the person allowed to ([`State`]),
//! and every plugin can be switched off or removed.
//!
//! This module knows nothing of the engine or the interface; validation takes a callback that says
//! whether a command exists.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::procedures::{self, Procedure};

/// The file format version this build reads and writes.
pub const FORMAT: u32 = 1;
/// File extension of a plugin.
pub const EXTENSION: &str = "tcgplugin";
/// The largest plugin file read, in bytes.
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_PROCEDURES: usize = 100;
pub const MAX_ITEMS: usize = 80;
pub const MAX_LISP_FILES: usize = 8;
pub const MAX_LISP_BYTES: usize = 100_000;
pub const MAX_INSTALLED: usize = 50;

/// A menu entry that runs one of the plugin's procedures.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MenuItem {
    pub label: String,
    /// The id of one of the plugin's procedures.
    pub procedure: String,
}

/// A LISP file loaded when the application starts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LispFile {
    pub name: String,
    pub code: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plugin {
    /// Format version of the file.
    #[serde(default = "format")]
    pub format: u32,
    /// Lower-case letters, digits and dashes; names the file.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    /// Title of the menu the items go in; empty for a plugin with a toolbar only.
    #[serde(default)]
    pub menu: String,
    #[serde(default)]
    pub items: Vec<MenuItem>,
    /// Ids of the procedures shown as buttons on the plugin's toolbar.
    #[serde(default)]
    pub toolbar: Vec<String>,
    #[serde(default)]
    pub procedures: Vec<Procedure>,
    #[serde(default)]
    pub lisp: Vec<LispFile>,
}

fn format() -> u32 {
    FORMAT
}

impl Plugin {
    pub fn procedure(&self, id: &str) -> Option<&Procedure> {
        self.procedures.iter().find(|p| p.id == id)
    }

    /// The menu title to show: the plugin's own, or `title (plugin)` when it would take a built-in
    /// menu's name (`reserved`).
    pub fn menu_title(&self, reserved: &[&str]) -> Option<String> {
        let t = self.menu.trim();
        if t.is_empty() || self.items.is_empty() {
            return None;
        }
        Some(if reserved.iter().any(|r| r.eq_ignore_ascii_case(t)) { format!("{t} (plugin)") } else { t.to_string() })
    }

    /// The text of the file.
    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

/// Read a plugin from the text of a file.
pub fn parse(text: &str) -> Result<Plugin, String> {
    if text.len() as u64 > MAX_BYTES {
        return Err(format!("el archivo pesa más de {} MB", MAX_BYTES / 1024 / 1024));
    }
    let p: Plugin = serde_json::from_str(text).map_err(|e| format!("no es un complemento válido: {e}"))?;
    if p.format > FORMAT {
        return Err(format!("el complemento es del formato {}; esta versión lee hasta el {FORMAT}. Actualiza TerraCAD-GIS", p.format));
    }
    Ok(p)
}

/// What is wrong with `p`, one sentence each; empty when it can be installed. `known` says whether a command is registered.
pub fn validate(p: &Plugin, known: impl Fn(&str) -> bool + Copy) -> Vec<String> {
    let mut bad = Vec::new();
    if p.id.is_empty() || p.id.len() > 48 || !p.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        bad.push(format!("el identificador «{}» solo puede tener minúsculas, números y guiones (máximo 48)", p.id));
    }
    if p.name.trim().is_empty() {
        bad.push("el complemento no tiene nombre".into());
    }
    if p.menu.chars().count() > 40 || p.menu.chars().any(char::is_control) {
        bad.push("el título del menú es demasiado largo o tiene caracteres de control".into());
    }
    if p.procedures.len() > MAX_PROCEDURES {
        bad.push(format!("tiene {} procedimientos; el máximo es {MAX_PROCEDURES}", p.procedures.len()));
    }
    if p.items.len() > MAX_ITEMS {
        bad.push(format!("tiene {} ítems de menú; el máximo es {MAX_ITEMS}", p.items.len()));
    }
    if p.lisp.len() > MAX_LISP_FILES {
        bad.push(format!("tiene {} archivos LISP; el máximo es {MAX_LISP_FILES}", p.lisp.len()));
    }
    let mut seen = BTreeSet::new();
    for pr in &p.procedures {
        if !seen.insert(pr.id.as_str()) {
            bad.push(format!("el procedimiento «{}» está repetido", pr.id));
        }
        bad.extend(procedures::validate(pr, known).into_iter().map(|m| format!("procedimiento «{}»: {m}", pr.id)));
    }
    for it in &p.items {
        if it.label.trim().is_empty() || it.label.chars().count() > 60 || it.label.chars().any(char::is_control) {
            bad.push(format!("el ítem «{}» no tiene un nombre válido", it.label));
        }
        if p.procedure(&it.procedure).is_none() {
            bad.push(format!("el ítem «{}» usa el procedimiento «{}», que no está en el complemento", it.label, it.procedure));
        }
    }
    for id in &p.toolbar {
        if p.procedure(id).is_none() {
            bad.push(format!("la barra de herramientas usa «{id}», que no es un procedimiento del complemento"));
        }
    }
    for l in &p.lisp {
        if l.name.trim().is_empty() || l.name.contains(['/', '\\']) {
            bad.push(format!("el archivo LISP «{}» no tiene un nombre válido", l.name));
        }
        if l.code.len() > MAX_LISP_BYTES {
            bad.push(format!("el archivo LISP «{}» pasa de {MAX_LISP_BYTES} bytes", l.name));
        } else if !procedures::balanced(&l.code) {
            bad.push(format!("los paréntesis del archivo LISP «{}» no cuadran", l.name));
        }
    }
    bad
}

/// A plugin made from procedures of the library, ready to share.
pub fn from_procedures(id: &str, name: &str, menu: &str, procs: Vec<Procedure>) -> Plugin {
    let items = procs.iter().map(|p| MenuItem { label: p.name.clone(), procedure: p.id.clone() }).collect();
    let toolbar = procs.iter().filter(|p| p.toolbar).map(|p| p.id.clone()).collect();
    let procedures = procs
        .into_iter()
        .map(|mut p| {
            p.builtin = false;
            p
        })
        .collect();
    Plugin {
        format: FORMAT,
        id: procedures::slug(id),
        name: name.trim().to_string(),
        version: "1.0".into(),
        author: String::new(),
        description: String::new(),
        menu: menu.trim().to_string(),
        items,
        toolbar,
        procedures,
        lisp: Vec::new(),
    }
}

/// What the person decided about the installed plugins.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Plugins switched off: no menu, no toolbar, no startup code.
    #[serde(default)]
    pub disabled: Vec<String>,
    /// Plugins whose startup LISP may run.
    #[serde(default)]
    pub startup: Vec<String>,
}

impl State {
    pub fn enabled(&self, id: &str) -> bool {
        !self.disabled.iter().any(|d| d == id)
    }

    pub fn may_run_startup(&self, id: &str) -> bool {
        self.enabled(id) && self.startup.iter().any(|s| s == id)
    }

    pub fn set_enabled(&mut self, id: &str, on: bool) {
        self.disabled.retain(|d| d != id);
        if !on {
            self.disabled.push(id.to_string());
        }
    }

    pub fn set_startup(&mut self, id: &str, on: bool) {
        self.startup.retain(|s| s != id);
        if on {
            self.startup.push(id.to_string());
        }
    }
}

/// The plugins folder's content: what is installed and the person's choices.
#[derive(Clone, Debug, Default)]
pub struct Installed {
    pub plugins: Vec<Plugin>,
    pub state: State,
    /// Files that could not be read, with why.
    pub problems: Vec<String>,
}

fn state_file(dir: &Path) -> PathBuf {
    dir.join("state.json")
}

fn plugin_file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.{EXTENSION}"))
}

impl Installed {
    /// Read every `*.tcgplugin` in `dir`. A missing folder is no plugins; a file that does not
    /// read is listed in `problems` and left alone.
    pub fn load(dir: &Path, known: impl Fn(&str) -> bool + Copy) -> Installed {
        let mut out = Installed::default();
        if let Ok(b) = std::fs::read(state_file(dir)) {
            out.state = serde_json::from_slice(&b).unwrap_or_default();
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return out };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == EXTENSION)).collect();
        files.sort();
        for f in files.into_iter().take(MAX_INSTALLED) {
            let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let read = std::fs::metadata(&f)
                .map_err(|e| e.to_string())
                .and_then(|m| if m.len() > MAX_BYTES { Err("es demasiado grande".to_string()) } else { Ok(()) })
                .and_then(|()| std::fs::read_to_string(&f).map_err(|e| e.to_string()))
                .and_then(|t| parse(&t));
            match read {
                Ok(p) => {
                    let why = validate(&p, known);
                    if why.is_empty() {
                        out.plugins.push(p);
                    } else {
                        out.problems.push(format!("{name}: {}", why.join("; ")));
                    }
                }
                Err(e) => out.problems.push(format!("{name}: {e}")),
            }
        }
        out
    }

    pub fn get(&self, id: &str) -> Option<&Plugin> {
        self.plugins.iter().find(|p| p.id == id)
    }

    /// The enabled plugins.
    pub fn active(&self) -> impl Iterator<Item = &Plugin> {
        self.plugins.iter().filter(|p| self.state.enabled(&p.id))
    }

    pub fn save_state(&self, dir: &Path) -> Result<(), String> {
        write_atomic(&state_file(dir), &serde_json::to_vec_pretty(&self.state).map_err(|e| e.to_string())?)
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| e.to_string())
}

/// Install `p` into `dir`, replacing an older version with the same id. Returns whether it replaced one.
pub fn install(dir: &Path, p: &Plugin, known: impl Fn(&str) -> bool + Copy) -> Result<bool, String> {
    let why = validate(p, known);
    if !why.is_empty() {
        return Err(why.join("\n"));
    }
    let path = plugin_file(dir, &p.id);
    let replaced = path.exists();
    if !replaced && Installed::load(dir, known).plugins.len() >= MAX_INSTALLED {
        return Err(format!("ya hay {MAX_INSTALLED} complementos instalados"));
    }
    write_atomic(&path, p.to_text().as_bytes())?;
    Ok(replaced)
}

/// Remove the plugin `id` and what was decided about it.
pub fn remove(dir: &Path, id: &str) -> Result<(), String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err("identificador no válido".into());
    }
    match std::fs::remove_file(plugin_file(dir, id)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    let mut state = std::fs::read(state_file(dir)).ok().and_then(|b| serde_json::from_slice::<State>(&b).ok()).unwrap_or_default();
    state.disabled.retain(|d| d != id);
    state.startup.retain(|s| s != id);
    write_atomic(&state_file(dir), &serde_json::to_vec_pretty(&state).map_err(|e| e.to_string())?)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::procedures::Step;

    fn known(c: &str) -> bool {
        matches!(c, "civil.tin" | "lisp" | "line")
    }

    fn proc(id: &str) -> Procedure {
        Procedure {
            id: id.into(),
            name: format!("Proc {id}"),
            description: String::new(),
            steps: vec![Step { command: "civil.tin".into(), params: json!({}) }],
            icon: "T".into(),
            toolbar: true,
            created: 0,
            notes: String::new(),
            builtin: false,
        }
    }

    fn plugin() -> Plugin {
        from_procedures("topo-extra", "Topografía extra", "Topo extra", vec![proc("a"), proc("b")])
    }

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tcg-plug-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_plugin_made_from_procedures_is_valid_and_round_trips() {
        let p = plugin();
        assert!(validate(&p, known).is_empty(), "{:?}", validate(&p, known));
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.toolbar, ["a", "b"]);
        assert_eq!(parse(&p.to_text()).unwrap(), p);
    }

    #[test]
    fn validation_names_each_problem() {
        let mut p = plugin();
        p.id = "Mal Id".into();
        p.items.push(MenuItem { label: "Roto".into(), procedure: "nope".into() });
        p.toolbar.push("tampoco".into());
        p.procedures[0].steps[0].command = "no-existe".into();
        p.lisp.push(LispFile { name: "../x.lsp".into(), code: "(defun f (".into() });
        let why = validate(&p, known).join("\n");
        for needle in ["identificador", "«nope»", "«tampoco»", "no-existe", "archivo LISP «../x.lsp» no tiene un nombre válido", "paréntesis"]
        {
            assert!(why.contains(needle), "{needle} in {why}");
        }
    }

    #[test]
    fn newer_formats_and_junk_are_refused_with_a_reason() {
        let mut p = plugin();
        p.format = FORMAT + 1;
        assert!(parse(&p.to_text()).unwrap_err().contains("Actualiza"));
        assert!(parse("{no json").is_err());
        assert!(parse(&"x".repeat(MAX_BYTES as usize + 1)).is_err());
    }

    #[test]
    fn menu_titles_avoid_the_built_in_ones() {
        let mut p = plugin();
        assert_eq!(p.menu_title(&["File", "Edit"]).as_deref(), Some("Topo extra"));
        p.menu = "edit".into();
        assert_eq!(p.menu_title(&["File", "Edit"]).as_deref(), Some("edit (plugin)"));
        p.menu.clear();
        assert_eq!(p.menu_title(&[]), None);
    }

    #[test]
    fn installing_replaces_loads_and_removes() {
        let d = dir("inst");
        assert!(Installed::load(&d, known).plugins.is_empty());
        let mut p = plugin();
        assert!(!install(&d, &p, known).unwrap());
        p.version = "2.0".into();
        assert!(install(&d, &p, known).unwrap(), "same id replaces");
        let mut inst = Installed::load(&d, known);
        assert_eq!(inst.plugins.len(), 1);
        assert_eq!(inst.get("topo-extra").unwrap().version, "2.0");
        inst.state.set_enabled("topo-extra", false);
        inst.state.set_startup("topo-extra", true);
        inst.save_state(&d).unwrap();
        let again = Installed::load(&d, known);
        assert_eq!(again.active().count(), 0);
        assert!(!again.state.may_run_startup("topo-extra"), "a disabled plugin runs nothing");
        remove(&d, "topo-extra").unwrap();
        let gone = Installed::load(&d, known);
        assert!(gone.plugins.is_empty() && gone.state == State::default());
        assert!(remove(&d, "../x").is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_bad_file_is_reported_and_does_not_hide_the_good_ones() {
        let d = dir("bad");
        install(&d, &plugin(), known).unwrap();
        std::fs::write(d.join("roto.tcgplugin"), "{no").unwrap();
        let mut worse = plugin();
        worse.id = "otro".into();
        worse.items[0].procedure = "falta".into();
        std::fs::write(d.join("otro.tcgplugin"), worse.to_text()).unwrap();
        let inst = Installed::load(&d, known);
        assert_eq!(inst.plugins.len(), 1);
        assert_eq!(inst.problems.len(), 2, "{:?}", inst.problems);
        assert!(install(&d, &worse, known).is_err(), "an invalid plugin is not installed");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_example_plugin_is_valid() {
        let text = include_str!("../../../examples/plugins/topografia-rapida.tcgplugin");
        let p = parse(text).unwrap();
        assert!(validate(&p, |c| c.starts_with("civil.")).is_empty());
        assert_eq!(p.items.len(), 2);
    }

    #[test]
    fn startup_code_needs_the_persons_permission() {
        let mut s = State::default();
        assert!(s.enabled("x") && !s.may_run_startup("x"));
        s.set_startup("x", true);
        assert!(s.may_run_startup("x"));
        s.set_startup("x", false);
        assert!(!s.may_run_startup("x"));
    }
}
