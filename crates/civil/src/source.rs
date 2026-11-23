//! The application's own source code, offered to the AI assistant.
//!
//! The assistant can only improve TerraCAD-GIS if it can read it. This module finds the source
//! folder (set by the person, or next to the program, or where it was built), builds a map of
//! the files, picks the ones that bear on a question, and packs everything an outside tool
//! (Claude Code, Antigravity, Cursor...) needs to carry on the work: the project's rules, the
//! map, the request, and the procedure or patch to refine.
//!
//! Only source text is read: the extensions in [`EXTENSIONS`], files up to [`MAX_FILE`] bytes,
//! inside the folder, never through `..` or a link that leaves it, and never the folders in
//! [`SKIP_DIRS`]. Nothing here writes into the source folder.

use std::path::{Component, Path, PathBuf};

use crate::procedures::{self, Procedure};

/// File types that count as source.
pub const EXTENSIONS: &[&str] = &["rs", "toml", "md", "ps1", "sh", "wxs", "wxi", "json", "jsonl", "lsp"];
/// Folders never read: build output, history, packages and bulky assets.
pub const SKIP_DIRS: &[&str] = &["target", ".git", "dist", "node_modules", "assets", "licenses", ".claude", "plan"];
/// The largest file read, in bytes.
pub const MAX_FILE: u64 = 200_000;
/// The most files listed in a map.
pub const MAX_LISTED: usize = 3000;

/// Whether `dir` is the root of the TerraCAD-GIS source.
pub fn is_root(dir: &Path) -> bool {
    dir.join("Cargo.toml").is_file() && dir.join("crates/civil/src/ai.rs").is_file()
}

/// Find the source folder: `hint` (what the person set), the `TERRACAD_SOURCE` variable, a
/// `source` folder beside the program, the program's parents (a build in `target/`), then where
/// this program was compiled.
pub fn find_root(hint: &str) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if !hint.trim().is_empty() {
        candidates.push(PathBuf::from(hint.trim()));
    }
    if let Ok(v) = std::env::var("TERRACAD_SOURCE")
        && !v.trim().is_empty()
    {
        candidates.push(PathBuf::from(v.trim()));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        candidates.push(dir.join("source"));
        candidates.extend(dir.ancestors().skip(1).take(5).map(Path::to_path_buf));
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));
    candidates.into_iter().find(|c| is_root(c)).map(|c| c.canonicalize().unwrap_or(c))
}

/// A source file in the map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileInfo {
    /// Path from the root, with `/`.
    pub path: String,
    pub bytes: u64,
    /// The first line of the file's `//!` documentation (or heading), when it has one.
    pub summary: String,
}

fn wanted(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.contains(&e))
}

/// The source files under `root`, sorted by path.
pub fn list_files(root: &Path) -> Vec<FileInfo> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(t) = e.file_type() else { continue };
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if t.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                    stack.push(path);
                }
            } else if t.is_file() && wanted(&path) && out.len() < MAX_LISTED {
                let bytes = e.metadata().map_or(0, |m| m.len());
                let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                out.push(FileInfo { summary: summary_of(&path, bytes), path: rel, bytes });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// The first `//!` line (or Markdown heading) of a small file.
fn summary_of(path: &Path, bytes: u64) -> String {
    if bytes > MAX_FILE {
        return String::new();
    }
    let Ok(text) = std::fs::read_to_string(path) else { return String::new() };
    for line in text.lines().take(12) {
        let t = line.trim();
        if let Some(d) = t.strip_prefix("//!") {
            return d.trim().chars().take(110).collect();
        }
        if let Some(h) = t.strip_prefix("# ") {
            return h.trim().chars().take(110).collect();
        }
    }
    String::new()
}

/// Read `rel` inside `root`. Refused: paths that climb out (`..`, absolute, a link out), other
/// file types, and files over [`MAX_FILE`].
pub fn read_file(root: &Path, rel: &str) -> Result<String, String> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() || rel_path.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err(format!("«{rel}» no está dentro de la carpeta del código fuente"));
    }
    if !wanted(rel_path) {
        return Err(format!("«{rel}» no es un archivo de código fuente"));
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let full = root.join(rel_path).canonicalize().map_err(|e| format!("«{rel}»: {e}"))?;
    if !full.starts_with(&root) {
        return Err(format!("«{rel}» sale de la carpeta del código fuente"));
    }
    if full.components().any(|c| matches!(c, Component::Normal(n) if SKIP_DIRS.iter().any(|d| n == std::ffi::OsStr::new(d)))) {
        return Err(format!("«{rel}» está en una carpeta que el asistente no lee"));
    }
    let len = std::fs::metadata(&full).map_err(|e| e.to_string())?.len();
    if len > MAX_FILE {
        return Err(format!("«{rel}» pesa {} KB; el máximo es {} KB", len / 1024, MAX_FILE / 1024));
    }
    std::fs::read_to_string(&full).map_err(|e| format!("«{rel}»: {e}"))
}

/// The map as Markdown: one line per file, grouped by crate or app folder.
pub fn repo_map(files: &[FileInfo]) -> String {
    let mut s = String::new();
    for f in files {
        s.push_str(&format!(
            "- `{}` ({} KB){}\n",
            f.path,
            f.bytes.div_ceil(1024),
            if f.summary.is_empty() { String::new() } else { format!(" — {}", f.summary) }
        ));
    }
    s
}

fn words(text: &str) -> Vec<String> {
    let mut w: Vec<String> =
        text.to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '_').filter(|w| w.chars().count() >= 4).map(str::to_string).collect();
    w.sort();
    w.dedup();
    w
}

/// The files that bear most on `question`: by words in the path and summary, then in the text.
pub fn relevant(root: &Path, files: &[FileInfo], question: &str, max: usize) -> Vec<String> {
    let ws = words(question);
    if ws.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, &FileInfo)> = files
        .iter()
        .filter(|f| f.bytes <= MAX_FILE && f.path.ends_with(".rs"))
        .map(|f| {
            let head = format!("{} {}", f.path.to_lowercase(), f.summary.to_lowercase());
            let body = std::fs::read_to_string(root.join(&f.path)).unwrap_or_default().to_lowercase();
            let score: usize = ws.iter().map(|w| 6 * usize::from(head.contains(w.as_str())) + body.matches(w.as_str()).count().min(5)).sum();
            (score, f)
        })
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
    scored.into_iter().take(max).map(|(_, f)| f.path.clone()).collect()
}

/// Cut `text` to at most `max` bytes at a line end, saying so.
fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let end = text[..end].rfind('\n').unwrap_or(end);
    format!("{}\n… (recortado: el archivo sigue)\n", &text[..end])
}

/// The rules every outside tool must follow, short.
pub const RULES: &str = "\
- Lenguaje: Rust puro (egui). Sin JavaScript/TypeScript, Tauri ni Electron. `unsafe` prohibido.\n\
- Capas (`cargo xtask layers`): L0 geom/color; L1 doc; L2 constraints/fonts/render; L3 io; L4 engine; L5 ui-egui/mcp; luego apps. Lo de abajo no depende de egui.\n\
- Los comandos viven en `crates/engine/src/cmd/*` (`CommandSpec`, parámetros JSON). Los comandos solo de interfaz, en `crates/ui-egui/src/menus.rs` (`UI_COMMANDS`). Los paneles cambian el motor solo con comandos registrados (`app.run(id, params)`).\n\
- Civil-ING y el asistente están en `crates/civil` (ai.rs, procedures.rs, source.rs) y `crates/ui-egui/src/civil_ai.rs`.\n\
- Todo texto visible pasa por `tl!`/`tf!` con la clave en inglés; las traducciones van en `crates/ui-egui/src/i18n/*.tsv` (es y zh deben traducir todas las etiquetas nuevas; líneas añadidas con tabulador inicial).\n\
- Pruebas: `cargo test -p <crate>`; lint: `cargo clippy -p <crate> --all-targets -- -D warnings`; además `cargo xtask layers` y `cargo xtask assets`.\n\
- Cada comando nuevo debe clasificarse en `crates/engine/src/file_access/commands.rs`.\n\
- No copiar código ni recursos de otros productos; no usar nombres de marcas de terceros para funciones.\n\
- Las claves de API nunca se escriben en el código, en registros ni en archivos del repositorio.\n";

/// What goes to the assistant with a question when the person asked to include the source:
/// the rules, the map (clipped) and the most relevant files, within `budget` bytes in all.
pub fn prompt_context(root: &Path, question: &str, budget: usize) -> String {
    let files = list_files(root);
    let mut s = String::from(
        "\n\nCÓDIGO FUENTE DE LA APLICACIÓN (solo lectura; propón cambios como diff unificado o archivos completos con su ruta):\nReglas del proyecto:\n",
    );
    s.push_str(RULES);
    s.push_str("\nMapa de archivos:\n");
    s.push_str(&clip(&repo_map(&files), budget / 3));
    let mut left = budget.saturating_sub(s.len());
    for rel in relevant(root, &files, question, 8) {
        if left < 2000 {
            break;
        }
        let Ok(text) = read_file(root, &rel) else { continue };
        let part = format!("\n--- {rel} ---\n{}", clip(&text, (left / 2).max(2000)));
        left = left.saturating_sub(part.len());
        s.push_str(&part);
    }
    s
}

/// The Markdown package to hand to an outside tool to continue the work: what the application
/// is, the rules, how to check, the request, the procedure (when there is one) with the patch
/// that would promote it, and the map of the source.
pub fn context_package(root: &Path, request: &str, procedure: Option<&Procedure>) -> String {
    let files = list_files(root);
    let mut s = String::from("# TerraCAD-GIS: contexto para continuar con un asistente de código\n\n");
    s.push_str("TerraCAD-GIS es una aplicación CAD de escritorio en Rust (base: CADCraft, MIT OR Apache-2.0) con el módulo de ingeniería civil Civil-ING y un Asistente IA. ");
    s.push_str("Lee `AGENTS.md` y `CLAUDE.md` en la raíz del repositorio antes de cambiar nada.\n\n## Reglas\n\n");
    s.push_str(RULES);
    s.push_str("\n## Qué se pide\n\n");
    s.push_str(if request.trim().is_empty() { "(sin descripción: escríbela aquí)" } else { request.trim() });
    s.push('\n');
    if let Some(p) = procedure {
        s.push_str(&format!("\n## Procedimiento «{}» (`{}`)\n\n{}\n\nPasos:\n\n", p.name, p.id, p.description));
        for (i, st) in p.steps.iter().enumerate() {
            s.push_str(&format!("{}. `{}`\n", i + 1, st.summary()));
        }
        s.push_str("\nJSON completo (una línea de `crates/civil/data/procedures.jsonl`):\n\n```json\n");
        s.push_str(&procedures::to_jsonl(p));
        s.push_str("\n```\n");
        let current = std::fs::read_to_string(root.join(procedures::BUNDLE_PATH)).unwrap_or_default();
        match procedures::promotion_patch(p, &current) {
            Ok(patch) => s.push_str(&format!("\nParche que lo incorpora como procedimiento permanente (`git apply`):\n\n```diff\n{patch}```\n")),
            Err(e) => s.push_str(&format!("\nNo se generó el parche: {e}\n")),
        }
    }
    s.push_str("\n## Cómo verificar\n\n```\ncargo fmt --all\ncargo test -p cadcraft-civil -p cadcraft-engine -p cadcraft-ui-egui\ncargo clippy -p cadcraft-civil --all-targets -- -D warnings\ncargo xtask layers\ncargo xtask assets\n```\n");
    s.push_str("\n## Mapa del código fuente\n\n");
    s.push_str(&repo_map(&files));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small tree shaped like the project.
    fn tree(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("tcg-src-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (p, text) in [
            ("Cargo.toml", "[workspace]\n"),
            ("crates/civil/src/ai.rs", "//! The assistant: talks to the services.\nfn chat() {}\n"),
            ("crates/civil/src/contour.rs", "//! Contour lines from a TIN.\nfn contours_interval() { /* curvas de nivel */ }\n"),
            ("target/debug/junk.rs", "fn junk() {}"),
            (".env", "SECRET=1"),
            ("README.md", "# TerraCAD-GIS\n"),
        ] {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, text).unwrap();
        }
        root
    }

    #[test]
    fn the_map_lists_source_and_skips_build_output_and_secrets() {
        let root = tree("map");
        assert!(is_root(&root));
        let files = list_files(&root);
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["Cargo.toml", "README.md", "crates/civil/src/ai.rs", "crates/civil/src/contour.rs"]);
        assert_eq!(files[2].summary, "The assistant: talks to the services.");
        assert!(repo_map(&files).contains("crates/civil/src/contour.rs"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reading_stays_inside_the_folder() {
        let root = tree("read");
        assert!(read_file(&root, "crates/civil/src/ai.rs").unwrap().contains("fn chat"));
        for bad in ["../x.rs", "/etc/passwd", "crates/../../x.rs", ".env", "target/debug/junk.rs", "crates/civil/src/missing.rs"] {
            assert!(read_file(&root, bad).is_err(), "{bad} must be refused");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_question_picks_the_files_about_it() {
        let root = tree("rel");
        let files = list_files(&root);
        assert_eq!(
            relevant(&root, &files, "mejora las curvas de nivel y los contours", 3).first().map(String::as_str),
            Some("crates/civil/src/contour.rs")
        );
        assert!(relevant(&root, &files, "zzzz qqqq", 3).is_empty());
        let ctx = prompt_context(&root, "contours interval", 20_000);
        assert!(ctx.contains("Reglas del proyecto") && ctx.contains("--- crates/civil/src/contour.rs ---"));
        assert!(!ctx.contains("SECRET"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_package_carries_the_request_the_procedure_and_a_patch() {
        let root = tree("pkg");
        let p = Procedure {
            id: "tin-rapido".into(),
            name: "TIN rápido".into(),
            description: "Triangula".into(),
            steps: vec![procedures::Step { command: "civil.tin".into(), params: serde_json::json!({"max_edge": 40}) }],
            icon: "TIN".into(),
            toolbar: true,
            created: 0,
            notes: String::new(),
            builtin: false,
        };
        std::fs::create_dir_all(root.join("crates/civil/data")).unwrap();
        std::fs::write(root.join(procedures::BUNDLE_PATH), "# vacío\n").unwrap();
        let pkg = context_package(&root, "Convertir en comando", Some(&p));
        assert!(pkg.contains("Convertir en comando") && pkg.contains("tin-rapido") && pkg.contains("```diff") && pkg.contains("cargo xtask layers"));
        assert!(!context_package(&root, "", None).contains("```diff"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn long_text_is_clipped_at_a_line() {
        let t = "linea uno\nlinea dos\nlinea tres\n";
        assert!(clip(t, 15).starts_with("linea uno\n"));
        assert!(clip(t, 15).contains("recortado"));
        assert_eq!(clip(t, 100), t);
    }
}
