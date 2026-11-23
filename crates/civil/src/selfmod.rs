//! Changes the assistant proposes to the application's own source code, and keeping them safe.
//!
//! Nothing here runs by itself: every function is called when the person presses a button. A change
//! comes as a unified diff in the assistant's reply; [`check_patch`] shows what it touches, [`apply_patch`]
//! applies it to the source folder and records it as a git commit of its own (so it can be reverted),
//! and the change is remembered in a small "pending" file until the program is rebuilt and restarted
//! ([`Pending`]). [`save_progress`] sends the work to a GitHub repository with the person's token.
//!
//! The token never appears in an argument list, a log or an error: git receives it through environment
//! variables, and every text this module returns is scrubbed of it first.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::ai::{AiError, base64};

/// The largest patch accepted, in bytes.
pub const MAX_PATCH: usize = 512 * 1024;

/// A change applied to the source that the running program does not have yet.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Change {
    pub title: String,
    /// The git commit that holds it.
    pub commit: String,
    pub files: Vec<String>,
}

/// Changes waiting for a rebuild and a restart; kept in a file so they survive closing the program.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Pending {
    pub changes: Vec<Change>,
}

impl Pending {
    /// Read the pending file (empty when there is none).
    pub fn load(file: &Path) -> Pending {
        std::fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Write the pending file; an empty list removes it.
    pub fn save(&self, file: &Path) -> std::io::Result<()> {
        if self.changes.is_empty() {
            return match std::fs::remove_file(file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(file, serde_json::to_vec_pretty(self).unwrap_or_default())
    }
}

/// The unified diffs in an assistant reply: code blocks tagged `diff` or `patch`, or any block that
/// looks like a git/unified diff.
pub fn patches_in(reply: &str) -> Vec<String> {
    crate::ai::code_blocks(reply)
        .into_iter()
        .filter(|b| matches!(b.lang.as_str(), "diff" | "patch") || looks_like_diff(&b.text))
        .map(|b| b.text)
        .collect()
}

/// Whether `text` has the headers of a unified diff.
pub fn looks_like_diff(text: &str) -> bool {
    text.lines().any(|l| l.starts_with("diff --git ")) || text.lines().any(|l| l.starts_with("--- ")) && text.lines().any(|l| l.starts_with("+++ "))
}

/// The files a patch touches (the `b/` side), without duplicates. Paths that leave the folder or enter
/// `.git` are an error.
pub fn patch_paths(patch: &str) -> Result<Vec<String>, AiError> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |p: &str| -> Result<(), AiError> {
        let p = p.trim().trim_matches('"');
        if p.is_empty() || p == "/dev/null" {
            return Ok(());
        }
        let bad = p.starts_with('/') || p.contains('\\') || p.contains(':') || p.split('/').any(|c| c == ".." || c.eq_ignore_ascii_case(".git"));
        if bad {
            return Err(AiError(format!("el parche toca una ruta no permitida: {p}")));
        }
        if !out.iter().any(|o| o == p) {
            out.push(p.to_string());
        }
        Ok(())
    };
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(i) = rest.rfind(" b/") {
                add(&rest[i + 3..])?;
            }
        } else if let Some(rest) = line.strip_prefix("+++ b/") {
            add(rest.split('\t').next().unwrap_or(rest))?;
        } else if let Some(rest) = line.strip_prefix("--- a/") {
            add(rest.split('\t').next().unwrap_or(rest))?;
        }
    }
    if out.is_empty() {
        return Err(AiError("no se reconoció ningún archivo en el parche".into()));
    }
    Ok(out)
}

/// What git printed, with `secrets` hidden.
fn scrub(text: &str, secrets: &[&str]) -> String {
    let mut t = text.to_string();
    for s in secrets.iter().filter(|s| s.len() >= 4) {
        t = t.replace(s, "***");
    }
    t
}

/// Run git in `root` with extra `env`; the output (both streams) on success, the scrubbed output on failure.
fn git(root: &Path, args: &[&str], env: &[(String, String)], stdin: Option<&str>, secrets: &[&str]) -> Result<String, AiError> {
    use std::io::Write;
    let mut cmd = Command::new("git");
    cmd.current_dir(root).args(args).env("GIT_TERMINAL_PROMPT", "0").stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // no console window
    }
    let mut child = cmd.spawn().map_err(|e| AiError(format!("no se pudo ejecutar git (¿está instalado y en el PATH?): {e}")))?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let _ = pipe.write_all(input.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| AiError(format!("git no terminó: {e}")))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let text = scrub(text.trim(), secrets);
    if out.status.success() { Ok(text) } else { Err(AiError(if text.is_empty() { "git falló sin mensaje".into() } else { text })) }
}

/// Whether `root` is inside a git work tree.
pub fn is_git(root: &Path) -> bool {
    git(root, &["rev-parse", "--is-inside-work-tree"], &[], None, &[]).is_ok_and(|t| t == "true")
}

/// Whether the patch applies cleanly to `root`; the answer is git's summary of what it changes.
pub fn check_patch(root: &Path, patch: &str) -> Result<String, AiError> {
    if patch.len() > MAX_PATCH {
        return Err(AiError("el parche es demasiado grande".into()));
    }
    patch_paths(patch)?;
    if !is_git(root) {
        return Err(AiError(
            "la carpeta del código fuente no es un repositorio de git; clona el proyecto con git para poder aplicar cambios con respaldo".into(),
        ));
    }
    let patch = with_newline(patch);
    git(root, &["apply", "--check", "--whitespace=nowarn", "-"], &[], Some(&patch), &[])?;
    git(root, &["apply", "--stat", "--whitespace=nowarn", "-"], &[], Some(&patch), &[])
}

fn with_newline(patch: &str) -> String {
    if patch.ends_with('\n') { patch.to_string() } else { format!("{patch}\n") }
}

/// Apply `patch` to the source in `root` and commit exactly the files it touches (other work in the folder
/// is left alone). Returns the change to remember.
pub fn apply_patch(root: &Path, patch: &str, title: &str) -> Result<Change, AiError> {
    check_patch(root, patch)?;
    let files = patch_paths(patch)?;
    let patch = with_newline(patch);
    git(root, &["apply", "--whitespace=nowarn", "-"], &[], Some(&patch), &[])?;
    let mut add: Vec<&str> = vec!["add", "--"];
    add.extend(files.iter().map(String::as_str));
    git(root, &add, &[], None, &[])?;
    let title = {
        let t: String = title.lines().next().unwrap_or("").trim().chars().take(72).collect();
        if t.is_empty() { "Cambio propuesto por el asistente".to_string() } else { t }
    };
    let message = format!("Asistente: {title}");
    // A computer with no git identity still gets a commit, made under the assistant's name.
    let has_identity = git(root, &["config", "user.email"], &[], None, &[]).is_ok_and(|t| !t.is_empty());
    let mut commit: Vec<&str> = Vec::new();
    if !has_identity {
        commit.extend(["-c", "user.name=Asistente TerraCAD-GIS", "-c", "user.email=asistente@terracad-gis.invalid"]);
    }
    commit.extend(["commit", "-m", &message, "--"]);
    commit.extend(files.iter().map(String::as_str));
    git(root, &commit, &[], None, &[])?;
    let hash = git(root, &["rev-parse", "--short", "HEAD"], &[], None, &[]).unwrap_or_default();
    Ok(Change { title, commit: hash, files })
}

/// A GitHub repository and the token that may write to it.
#[derive(Clone, Default)]
pub struct GitHub {
    /// `owner/repo`.
    pub repo: String,
    /// The branch the progress goes to.
    pub branch: String,
    /// A personal access token. Hidden from `Debug`.
    pub token: String,
}

impl std::fmt::Debug for GitHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHub").field("repo", &self.repo).field("branch", &self.branch).field("token", &"<hidden>").finish()
    }
}

/// The branch used when none is set.
pub const DEFAULT_BRANCH: &str = "terracad-gis-asistente";

impl GitHub {
    /// The branch to use.
    pub fn branch_or_default(&self) -> &str {
        let b = self.branch.trim();
        if b.is_empty() { DEFAULT_BRANCH } else { b }
    }

    /// Check what was typed, before anything is sent.
    pub fn validate(&self) -> Result<(), AiError> {
        let ok_name =
            |s: &str| !s.is_empty() && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        let repo = self.repo.trim();
        match repo.split_once('/') {
            Some((o, r)) if ok_name(o) && ok_name(r) => {}
            _ => return Err(AiError("el repositorio debe escribirse como propietario/nombre (por ejemplo storytold/cadcraft)".into())),
        }
        let b = self.branch_or_default();
        if b.starts_with('-')
            || b.starts_with('/')
            || b.ends_with('/')
            || b.ends_with(".lock")
            || b.contains("..")
            || b.contains("//")
            || !b.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
        {
            return Err(AiError("el nombre de la rama solo puede tener letras, números, punto, guion, guion bajo y barra".into()));
        }
        let t = self.token.trim();
        if t.is_empty() {
            return Err(AiError("falta el token de acceso de GitHub".into()));
        }
        if !t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')) {
            return Err(AiError("el token de GitHub solo puede tener letras, números, guion y guion bajo".into()));
        }
        Ok(())
    }

    fn url(&self) -> String {
        format!("https://github.com/{}.git", self.repo.trim())
    }
}

/// Check the token and the repository with GitHub's API: that the repository can be reached and that the token
/// may write to it. Blocks briefly.
#[cfg(not(target_arch = "wasm32"))]
pub fn test_github(gh: &GitHub) -> Result<String, AiError> {
    use std::io::Read;
    use std::time::{Duration, Instant};
    gh.validate()?;
    let started = Instant::now();
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).http_status_as_error(false).build().into();
    let resp = agent
        .get(&format!("https://api.github.com/repos/{}", gh.repo.trim()))
        .header("authorization", &format!("Bearer {}", gh.token.trim()))
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", "2022-11-28")
        .header("user-agent", "TerraCAD-GIS")
        .call()
        .map_err(|e| AiError(scrub(&format!("no se pudo contactar a GitHub: {e}"), &[gh.token.trim()])))?;
    let status = resp.status().as_u16();
    let mut body = String::new();
    resp.into_body()
        .into_reader()
        .take(1024 * 1024)
        .read_to_string(&mut body)
        .map_err(|e| AiError(format!("no se pudo leer la respuesta de GitHub: {e}")))?;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    match status {
        200 => {
            let push = v.pointer("/permissions/push").and_then(serde_json::Value::as_bool).unwrap_or(false);
            let private = v.get("private").and_then(serde_json::Value::as_bool).unwrap_or(false);
            let default = v.get("default_branch").and_then(serde_json::Value::as_str).unwrap_or("?");
            let name = v.get("full_name").and_then(serde_json::Value::as_str).unwrap_or(gh.repo.trim());
            let line =
                format!("{name} ({}), rama por defecto {default}, {} ms", if private { "privado" } else { "público" }, started.elapsed().as_millis());
            if push {
                Ok(format!("✔ {line}. El token puede escribir; el progreso irá a la rama «{}».", gh.branch_or_default()))
            } else {
                Err(AiError(format!("{line}. El token NO tiene permiso de escritura (Contents: Read and write).")))
            }
        }
        401 => Err(AiError("GitHub rechazó el token (401): está vencido o mal escrito".into())),
        403 | 429 => Err(AiError("GitHub negó el acceso o limitó las solicitudes (403/429); revisa los permisos del token".into())),
        404 => Err(AiError("GitHub no encontró el repositorio (404): revisa el nombre, o que el token tenga acceso a él".into())),
        s => Err(AiError(format!("GitHub respondió con el estado HTTP {s}"))),
    }
}

#[cfg(target_arch = "wasm32")]
pub fn test_github(_: &GitHub) -> Result<String, AiError> {
    Err(AiError("no disponible en la versión web".into()))
}

/// Commit everything changed in `root` (when anything is) and push the work to the repository's progress
/// branch. The push does not touch the person's branches or remotes.
pub fn save_progress(root: &Path, gh: &GitHub, message: &str) -> Result<String, AiError> {
    gh.validate()?;
    if !is_git(root) {
        return Err(AiError("la carpeta del código fuente no es un repositorio de git".into()));
    }
    let token = gh.token.trim();
    let basic = base64(format!("x-access-token:{token}").as_bytes());
    let secrets = [token, basic.as_str()];
    let env = vec![
        ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
        ("GIT_CONFIG_KEY_0".to_string(), "http.https://github.com/.extraheader".to_string()),
        ("GIT_CONFIG_VALUE_0".to_string(), format!("AUTHORIZATION: basic {basic}")),
    ];
    let mut report = Vec::new();
    let dirty = !git(root, &["status", "--porcelain"], &[], None, &[])?.is_empty();
    if dirty {
        git(root, &["add", "-A"], &[], None, &secrets)?;
        let line: String = message.lines().next().unwrap_or("").trim().chars().take(72).collect();
        let message = if line.is_empty() { "Avance guardado desde el asistente de TerraCAD-GIS".to_string() } else { line };
        let has_identity = git(root, &["config", "user.email"], &[], None, &[]).is_ok_and(|t| !t.is_empty());
        let mut commit: Vec<&str> = Vec::new();
        if !has_identity {
            commit.extend(["-c", "user.name=Asistente TerraCAD-GIS", "-c", "user.email=asistente@terracad-gis.invalid"]);
        }
        commit.extend(["commit", "-m", &message]);
        git(root, &commit, &[], None, &secrets)?;
        report.push(format!("Se guardó un commit: {message}"));
    } else {
        report.push("No había cambios nuevos por guardar.".to_string());
    }
    let refspec = format!("HEAD:refs/heads/{}", gh.branch_or_default());
    let url = gh.url();
    let out = git(root, &["push", &url, &refspec], &env, None, &secrets)?;
    report.push(format!("Enviado a {} (rama {}).", gh.repo.trim(), gh.branch_or_default()));
    if !out.is_empty() {
        report.push(out.lines().last().unwrap_or("").to_string());
    }
    Ok(report.join(" "))
}

/// The program `cargo`, when it can be run.
pub fn has_cargo() -> bool {
    Command::new("cargo").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// The release program a rebuild in `root` produces.
pub fn built_program(root: &Path) -> PathBuf {
    root.join("target").join("release").join(if cfg!(windows) { "terracad-gis.exe" } else { "terracad-gis" })
}

/// Rebuild the program from `root` (`cargo build --release -p cadcraft`), writing the compiler output to
/// `log`. Blocks for minutes: call it from a worker thread. Returns the program built.
pub fn rebuild(root: &Path, log: &Path) -> Result<PathBuf, AiError> {
    if !has_cargo() {
        return Err(AiError("no se encontró cargo (Rust) en este equipo; compila con compilar-windows.bat o instala Rust desde rustup.rs".into()));
    }
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let out = std::fs::File::create(log).map_err(|e| AiError(format!("no se pudo crear el registro de compilación: {e}")))?;
    let err = out.try_clone().map_err(|e| AiError(e.to_string()))?;
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root).args(["build", "--release", "-p", "cadcraft"]).stdout(out).stderr(err);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let status = cmd.status().map_err(|e| AiError(format!("no se pudo ejecutar cargo: {e}")))?;
    if !status.success() {
        return Err(AiError(format!("la compilación falló; el registro está en {}", log.display())));
    }
    let exe = built_program(root);
    if exe.is_file() { Ok(exe) } else { Err(AiError(format!("la compilación terminó pero no está {}", exe.display()))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "diff --git a/crates/x/src/lib.rs b/crates/x/src/lib.rs\nindex 111..222 100644\n--- a/crates/x/src/lib.rs\n+++ b/crates/x/src/lib.rs\n@@ -1 +1,2 @@\n a\n+b\n";

    #[test]
    fn finds_the_diffs_in_a_reply() {
        let reply = format!("Cambio:\n```diff\n{PATCH}```\nY esto:\n```rust\nfn main() {{}}\n```\n");
        let found = patches_in(&reply);
        assert_eq!(found.len(), 1);
        assert!(found[0].contains("+b"));
        assert!(looks_like_diff(PATCH));
        assert!(!looks_like_diff("fn main() {}"));
    }

    #[test]
    fn lists_the_files_and_refuses_unsafe_paths() {
        assert_eq!(patch_paths(PATCH).unwrap(), vec!["crates/x/src/lib.rs"]);
        let new = "diff --git a/n.rs b/n.rs\nnew file mode 100644\n--- /dev/null\n+++ b/n.rs\n@@ -0,0 +1 @@\n+x\n";
        assert_eq!(patch_paths(new).unwrap(), vec!["n.rs"]);
        for bad in [
            "--- a/../etc/passwd\n+++ b/../etc/passwd\n",
            "--- a/.git/config\n+++ b/.git/config\n",
            "--- a//etc/x\n+++ b//etc/x\n",
            "--- a/C:/x\n+++ b/C:/x\n",
        ] {
            assert!(patch_paths(bad).is_err(), "{bad}");
        }
        assert!(patch_paths("nada").is_err());
    }

    #[test]
    fn github_settings_are_checked_before_use() {
        let ok = GitHub { repo: "storytold/cadcraft".into(), branch: String::new(), token: "ghp_abc123".into() };
        assert!(ok.validate().is_ok());
        assert_eq!(ok.branch_or_default(), DEFAULT_BRANCH);
        for (repo, branch, token) in [
            ("sin-barra", "", "ghp_x"),
            ("a/b/c", "", "ghp_x"),
            ("../x", "", "ghp_x"),
            ("a/b", "--upload-pack=x", "ghp_x"),
            ("a/b", "x..y", "ghp_x"),
            ("a/b", "", ""),
            ("a/b", "", "tok en"),
        ] {
            let gh = GitHub { repo: repo.into(), branch: branch.into(), token: token.into() };
            assert!(gh.validate().is_err(), "{repo} {branch} {token}");
        }
        assert!(!format!("{ok:?}").contains("ghp_abc123"));
    }

    #[test]
    fn the_token_is_scrubbed_from_output() {
        assert_eq!(scrub("fatal: https://x:SECRET123@github.com", &["SECRET123"]), "fatal: https://x:***@github.com");
    }

    #[test]
    fn pending_changes_survive_in_a_file_and_an_empty_list_removes_it() {
        let dir = std::env::temp_dir().join(format!("tcg-pending-{}", std::process::id()));
        let file = dir.join("pending.json");
        let p = Pending { changes: vec![Change { title: "t".into(), commit: "abc".into(), files: vec!["a.rs".into()] }] };
        p.save(&file).unwrap();
        assert_eq!(Pending::load(&file), p);
        Pending::default().save(&file).unwrap();
        assert!(!file.exists());
        assert_eq!(Pending::load(&file), Pending::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_patch_is_applied_and_committed_in_a_real_repository() {
        let dir = std::env::temp_dir().join(format!("tcg-repo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("crates/x/src")).unwrap();
        let run = |args: &[&str]| git(&dir, args, &[], None, &[]);
        if run(&["init", "-q"]).is_err() {
            return; // no git here
        }
        run(&["config", "user.email", "t@t"]).unwrap();
        run(&["config", "user.name", "t"]).unwrap();
        std::fs::write(dir.join("crates/x/src/lib.rs"), "a\n").unwrap();
        std::fs::write(dir.join("otro.txt"), "no tocar\n").unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-qm", "base"]).unwrap();
        // Unrelated work in progress must stay out of the assistant's commit.
        std::fs::write(dir.join("otro.txt"), "no tocar, editado\n").unwrap();
        assert!(check_patch(&dir, PATCH).unwrap().contains("lib.rs"));
        let change = apply_patch(&dir, PATCH, "Agrega b").unwrap();
        assert_eq!(change.files, vec!["crates/x/src/lib.rs"]);
        assert_eq!(std::fs::read_to_string(dir.join("crates/x/src/lib.rs")).unwrap(), "a\nb\n");
        assert!(run(&["log", "-1", "--format=%s"]).unwrap().contains("Asistente: Agrega b"));
        assert!(run(&["status", "--porcelain"]).unwrap().contains("otro.txt"), "the other file is still uncommitted");
        // The same patch no longer applies.
        assert!(check_patch(&dir, PATCH).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
