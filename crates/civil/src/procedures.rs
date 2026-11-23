//! Procedures: sequences of commands the AI assistant proposed (or a recording became) that the
//! person keeps, runs again from a toolbar button, and can promote into the application.
//!
//! A procedure is a list of steps; a step is a registered command with JSON parameters (the same
//! form the menus, scripts and the automation interface use). A LISP routine is the step
//! `lisp {code}`. The library lives in the settings folder; the procedures promoted into the
//! source ship with the application as built-in ones ([`bundled`]). This module knows nothing of
//! the engine: it validates names through a callback and leaves running to the interface.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ai;

/// The most steps a procedure may have.
pub const MAX_STEPS: usize = 200;
/// The longest LISP step, in bytes.
pub const MAX_LISP: usize = 20_000;
/// The most procedures the personal library keeps.
pub const MAX_PROCEDURES: usize = 500;

/// One command of a procedure.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub command: String,
    #[serde(default)]
    pub params: Value,
}

impl Step {
    /// A LISP routine as a step.
    pub fn lisp(code: &str) -> Step {
        Step { command: "lisp".into(), params: json!({ "code": code }) }
    }

    /// A one-line description for lists.
    pub fn summary(&self) -> String {
        if self.command == "lisp" {
            let code = self.params.get("code").and_then(Value::as_str).unwrap_or("");
            let first = code.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
            let mut text: String = first.chars().take(60).collect();
            if first.chars().count() > 60 {
                text.push('…');
            }
            return format!("lisp  {text}");
        }
        if self.params.is_null() || self.params.as_object().is_some_and(|o| o.is_empty()) {
            return self.command.clone();
        }
        let mut shown = self.params.to_string();
        if shown.chars().count() > 70 {
            shown = shown.chars().take(70).collect::<String>() + "…";
        }
        format!("{}  {shown}", self.command)
    }
}

/// A saved procedure.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Procedure {
    /// Stable name for files and the patch: lower-case letters, digits and dashes.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub steps: Vec<Step>,
    /// Up to three characters for its toolbar button.
    #[serde(default)]
    pub icon: String,
    /// Shown on the assistant's toolbar.
    #[serde(default)]
    pub toolbar: bool,
    /// Seconds since 1970 when it was saved.
    #[serde(default)]
    pub created: u64,
    /// What the assistant or the person noted: why it exists, what was tested.
    #[serde(default)]
    pub notes: String,
    /// Ships with the application (set when loaded from [`bundled`], never saved).
    #[serde(default, skip_serializing)]
    pub builtin: bool,
}

/// A lower-case id from a name: letters and digits, words joined by dashes.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(fold) {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    if out.is_empty() { "procedimiento".into() } else { out.chars().take(48).collect() }
}

/// Spanish accents folded to plain letters for ids.
fn fold(c: char) -> impl Iterator<Item = char> {
    let r = match c {
        'á' | 'à' | 'ä' | 'Á' => 'a',
        'é' | 'è' | 'ë' | 'É' => 'e',
        'í' | 'ì' | 'ï' | 'Í' => 'i',
        'ó' | 'ò' | 'ö' | 'Ó' => 'o',
        'ú' | 'ù' | 'ü' | 'Ú' => 'u',
        'ñ' | 'Ñ' => 'n',
        o => o,
    };
    std::iter::once(r)
}

/// `id` made unique among `taken` by a numeric suffix.
pub fn unique_id(id: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == id) {
        return id.to_string();
    }
    (2..).map(|n| format!("{id}-{n}")).find(|c| !taken.iter().any(|t| t == c)).unwrap_or_else(|| id.to_string())
}

/// What is wrong with `p`, one sentence each; empty when it can be saved and run. `known` says
/// whether a command id is registered.
pub fn validate(p: &Procedure, known: impl Fn(&str) -> bool) -> Vec<String> {
    let mut bad = Vec::new();
    if p.name.trim().is_empty() {
        bad.push("el procedimiento no tiene nombre".to_string());
    }
    if p.id.is_empty() || !p.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        bad.push(format!("el identificador «{}» solo puede tener minúsculas, números y guiones", p.id));
    }
    if p.steps.is_empty() {
        bad.push("no tiene pasos".to_string());
    }
    if p.steps.len() > MAX_STEPS {
        bad.push(format!("tiene {} pasos; el máximo es {MAX_STEPS}", p.steps.len()));
    }
    for (i, s) in p.steps.iter().enumerate() {
        let n = i + 1;
        if !known(&s.command) {
            bad.push(format!("paso {n}: el comando «{}» no existe", s.command));
        }
        if !(s.params.is_null() || s.params.is_object()) {
            bad.push(format!("paso {n}: los parámetros deben ser un objeto JSON"));
        }
        if s.command == "lisp" {
            match s.params.get("code").and_then(Value::as_str) {
                None | Some("") => bad.push(format!("paso {n}: la rutina LISP está vacía")),
                Some(c) if c.len() > MAX_LISP => bad.push(format!("paso {n}: la rutina LISP pasa de {MAX_LISP} bytes")),
                Some(c) if !balanced(c) => bad.push(format!("paso {n}: los paréntesis de la rutina LISP no cuadran")),
                Some(_) => {}
            }
        }
    }
    bad
}

/// Whether the parentheses of LISP code close, leaving strings and `;` comments alone.
pub fn balanced(code: &str) -> bool {
    let (mut depth, mut in_str, mut comment, mut esc) = (0i64, false, false, false);
    for c in code.chars() {
        if comment {
            comment = c != '\n';
        } else if in_str {
            match (esc, c) {
                (true, _) => esc = false,
                (false, '\\') => esc = true,
                (false, '"') => in_str = false,
                _ => {}
            }
        } else {
            match c {
                '"' => in_str = true,
                ';' => comment = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth < 0 {
                        return false;
                    }
                }
                _ => {}
            }
        }
    }
    depth == 0 && !in_str
}

/// The steps in a reply, in the order the blocks come: commands from ```cadcraft blocks and
/// routines from ```lisp ones.
pub fn steps_from_reply(reply: &str) -> Vec<Step> {
    let mut out = Vec::new();
    for b in ai::code_blocks(reply) {
        match b.lang.as_str() {
            "cadcraft" => out.extend(ai::proposals_of(&b.text).into_iter().map(|p| Step { command: p.command, params: p.params })),
            "lisp" | "autolisp" | "lsp" if !b.text.trim().is_empty() => out.push(Step::lisp(b.text.trim_end())),
            _ => {}
        }
    }
    out
}

/// The personal library, kept in a JSON file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    pub procedures: Vec<Procedure>,
}

impl Library {
    pub fn ids(&self) -> Vec<String> {
        self.procedures.iter().map(|p| p.id.clone()).collect()
    }

    pub fn get(&self, id: &str) -> Option<&Procedure> {
        self.procedures.iter().find(|p| p.id == id)
    }

    /// Add `p`, giving it a free id when its own is taken. Err when the library is full.
    pub fn add(&mut self, mut p: Procedure) -> Result<String, String> {
        if self.procedures.len() >= MAX_PROCEDURES {
            return Err(format!("la biblioteca ya tiene {MAX_PROCEDURES} procedimientos"));
        }
        p.id = unique_id(&p.id, &self.ids());
        let id = p.id.clone();
        self.procedures.push(p);
        Ok(id)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let n = self.procedures.len();
        self.procedures.retain(|p| p.id != id);
        n != self.procedures.len()
    }

    /// Read the library at `path`; a missing file is an empty library, a damaged one an error.
    pub fn load(path: &Path) -> Result<Library, String> {
        match std::fs::read(path) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Library::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Write the library to `path`, through a temporary file so a failure keeps the old one.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| e.to_string())
    }
}

/// The procedures promoted into the application, from `data/procedures.jsonl`: one JSON
/// procedure per line; blank lines and lines starting with `#` are comments.
pub fn bundled() -> Vec<Procedure> {
    parse_jsonl(include_str!("../data/procedures.jsonl"))
}

/// The procedures in JSON-lines `text`, marked built-in. A line that does not parse is skipped.
pub fn parse_jsonl(text: &str) -> Vec<Procedure> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| serde_json::from_str::<Procedure>(l).ok())
        .map(|mut p| {
            p.builtin = true;
            p
        })
        .collect()
}

/// `p` as one line of `procedures.jsonl`.
pub fn to_jsonl(p: &Procedure) -> String {
    serde_json::to_string(p).unwrap_or_default()
}

/// Where the promoted procedures live in the source tree.
pub const BUNDLE_PATH: &str = "crates/civil/data/procedures.jsonl";

/// A unified diff that appends `p` to the bundle whose current text is `current`, ready for
/// `git apply`. Err when a procedure with that id is already in it.
pub fn promotion_patch(p: &Procedure, current: &str) -> Result<String, String> {
    if parse_jsonl(current).iter().any(|b| b.id == p.id) {
        return Err(format!("«{}» ya está en {BUNDLE_PATH}", p.id));
    }
    let lines: Vec<&str> = current.lines().collect();
    let ctx = &lines[lines.len().saturating_sub(3)..];
    let old_start = lines.len() - ctx.len() + 1;
    let mut out = format!("diff --git a/{BUNDLE_PATH} b/{BUNDLE_PATH}\n--- a/{BUNDLE_PATH}\n+++ b/{BUNDLE_PATH}\n");
    out.push_str(&format!("@@ -{old_start},{} +{old_start},{} @@\n", ctx.len(), ctx.len() + 1));
    for l in ctx {
        out.push(' ');
        out.push_str(l);
        out.push('\n');
    }
    out.push('+');
    out.push_str(&to_jsonl(p));
    out.push('\n');
    if !current.is_empty() && !current.ends_with('\n') {
        return Err(format!("{BUNDLE_PATH} no termina en salto de línea"));
    }
    Ok(out)
}

/// The question that turns a recording into a procedure proposal.
pub fn recording_prompt(lines: &[String]) -> String {
    let mut s = String::from(
        "Grabé estas acciones en TerraCAD-GIS (una por línea: `command` inicia un comando, `point x,y`, `keyword`, `text`, `pick` son sus entradas, `run id {json}` es un comando con parámetros). \
Analízalas, dime qué intenté hacer y propón un procedimiento reutilizable: bloques ```cadcraft con comandos y parámetros JSON, o una rutina ```lisp si la tarea necesita lógica o repetición. \
Generaliza lo que sea específico de este dibujo (coordenadas, nombres de capa) y avísame qué parámetros debería cambiar cada vez. Si algo falló (`; error:`), corrígelo.\n\n```text\n",
    );
    for l in lines {
        s.push_str(l);
        s.push('\n');
    }
    s.push_str("```\n");
    s
}

/// The question that sends a trial run's results back for correction.
pub fn test_report_prompt(name: &str, results: &[(String, Result<String, String>)], before: usize, after: usize) -> String {
    let mut s = format!(
        "Probé el procedimiento «{name}» sobre una copia del dibujo (después la deshice). Objetos antes: {before}, después: {after}.\nResultado por paso:\n"
    );
    for (i, (step, r)) in results.iter().enumerate() {
        match r {
            Ok(m) => s.push_str(&format!("{}. OK  {step}  → {m}\n", i + 1)),
            Err(e) => s.push_str(&format!("{}. ERROR  {step}  → {e}\n", i + 1)),
        }
    }
    s.push_str("Si hubo errores o el resultado no es el esperado, propón la versión corregida completa (bloques ```cadcraft o ```lisp). Si todo está bien, dímelo y sugiere un nombre y descripción para guardarlo.");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(id: &str, steps: Vec<Step>) -> Procedure {
        Procedure {
            id: id.into(),
            name: "Mi procedimiento".into(),
            description: String::new(),
            steps,
            icon: "MP".into(),
            toolbar: true,
            created: 1,
            notes: String::new(),
            builtin: false,
        }
    }

    #[test]
    fn slugs_fold_accents_and_punctuation() {
        assert_eq!(slug("Curvas de nivel — cada 0,5 m"), "curvas-de-nivel-cada-0-5-m");
        assert_eq!(slug("Cuadro de áreas Ñandú"), "cuadro-de-areas-nandu");
        assert_eq!(slug("¡¡"), "procedimiento");
        assert!(slug(&"a".repeat(200)).len() <= 48);
    }

    #[test]
    fn ids_get_a_suffix_when_taken() {
        let taken = vec!["x".to_string(), "x-2".to_string()];
        assert_eq!(unique_id("y", &taken), "y");
        assert_eq!(unique_id("x", &taken), "x-3");
    }

    #[test]
    fn steps_come_from_cadcraft_and_lisp_blocks_in_order() {
        let reply = "Primero:\n```cadcraft\n[{\"command\":\"civil.tin\",\"params\":{\"max_edge\":50}}]\n```\nluego\n```lisp\n(command \"LINE\" \"0,0\" \"1,1\" \"\")\n```\ny\n```rust\nfn x() {}\n```\n";
        let steps = steps_from_reply(reply);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].command, "civil.tin");
        assert_eq!(steps[1].command, "lisp");
        assert!(steps[1].params["code"].as_str().unwrap().contains("LINE"));
    }

    #[test]
    fn validation_names_each_problem() {
        let known = |c: &str| matches!(c, "civil.tin" | "lisp" | "line");
        assert!(validate(&proc("ok", vec![Step { command: "civil.tin".into(), params: json!({}) }]), known).is_empty());
        let bad = proc("Mal Id", vec![Step { command: "nope".into(), params: json!([1]) }, Step::lisp("(defun f (")]);
        let why = validate(&bad, known).join("\n");
        assert!(why.contains("identificador"), "{why}");
        assert!(why.contains("paso 1: el comando «nope» no existe"), "{why}");
        assert!(why.contains("objeto JSON"), "{why}");
        assert!(why.contains("paso 2") && why.contains("paréntesis"), "{why}");
        assert!(validate(&proc("vacio", vec![]), known).join("").contains("no tiene pasos"));
    }

    #[test]
    fn lisp_balance_ignores_strings_and_comments() {
        assert!(balanced("(a (b \"(\" c)) ; )"));
        assert!(!balanced("(a \"x)\""));
        assert!(!balanced(")("));
        assert!(balanced("(princ \"a\\\"(\")"));
    }

    #[test]
    fn the_library_round_trips_through_a_file() {
        let dir = std::env::temp_dir().join(format!("tcg-proc-{}", std::process::id()));
        let path = dir.join("procedures.json");
        let mut lib = Library::load(&path).unwrap();
        assert!(lib.procedures.is_empty());
        let id = lib.add(proc("a", vec![Step::lisp("(princ)")])).unwrap();
        assert_eq!(lib.add(proc("a", vec![Step::lisp("(princ)")])).unwrap(), "a-2");
        assert_eq!(id, "a");
        lib.save(&path).unwrap();
        let back = Library::load(&path).unwrap();
        assert_eq!(back, lib);
        assert!(!path.with_extension("json.tmp").exists());
        std::fs::write(&path, b"{not json").unwrap();
        assert!(Library::load(&path).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_promotion_patch_appends_one_line_and_applies() {
        let current = "# comentario\n# otro\n";
        let p = proc("nuevo", vec![Step { command: "civil.tin".into(), params: json!({}) }]);
        let patch = promotion_patch(&p, current).unwrap();
        // Apply the one hunk by hand: its context must match the end of the file.
        let mut lines: Vec<&str> = current.lines().collect();
        let body: Vec<&str> = patch.lines().skip_while(|l| !l.starts_with("@@")).skip(1).collect();
        let ctx: Vec<&str> = body.iter().filter(|l| l.starts_with(' ')).map(|l| &l[1..]).collect();
        assert_eq!(ctx, lines[lines.len() - ctx.len()..]);
        let added: Vec<&str> = body.iter().filter(|l| l.starts_with('+')).map(|l| &l[1..]).collect();
        assert_eq!(added.len(), 1);
        lines.extend(added.iter().copied());
        let text = lines.join("\n") + "\n";
        assert_eq!(parse_jsonl(&text).len(), 1);
        assert!(patch.contains("@@ -1,2 +1,3 @@"), "{patch}");
        assert!(promotion_patch(&p, &text).is_err(), "no duplicates");
    }

    #[test]
    fn the_bundle_parses_and_has_unique_ids() {
        let b = bundled();
        let mut ids: Vec<&str> = b.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), b.len());
        assert!(b.iter().all(|p| p.builtin));
    }

    #[test]
    fn prompts_carry_what_they_are_about() {
        let r = recording_prompt(&["command line".into(), "point 0,0".into()]);
        assert!(r.contains("command line") && r.contains("```text"));
        let t = test_report_prompt("x", &[("civil.tin".into(), Err("sin puntos".into()))], 3, 3);
        assert!(t.contains("ERROR") && t.contains("sin puntos") && t.contains("antes: 3"));
    }
}
