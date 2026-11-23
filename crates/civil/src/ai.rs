//! The AI assistant client: chat with Anthropic (Claude), Google (Gemini) or DeepSeek from inside TerraCAD-GIS.
//!
//! The user brings their own API key. The assistant works agentically but with a person in the
//! loop: its replies may contain commands (in ```cadcraft blocks) and code or improvement
//! proposals, and TerraCAD-GIS shows each one for the user to run or save. Nothing the model writes
//! is run on its own.
//!
//! The functions that build requests and read replies are pure, so they are tested without a
//! network; [`chat`] is the one call that goes out.

use serde_json::{Value, json};

/// The AI service to talk to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Provider {
    #[default]
    Anthropic,
    Gemini,
    DeepSeek,
    /// A server on this computer or network that speaks the OpenAI chat dialect: LM Studio,
    /// Ollama, llama.cpp, Núcleo IA...
    Local,
}

impl Provider {
    pub const ALL: [Provider; 4] = [Provider::Anthropic, Provider::Gemini, Provider::DeepSeek, Provider::Local];

    pub fn label(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic (Claude)",
            Provider::Gemini => "Google (Gemini)",
            Provider::DeepSeek => "DeepSeek",
            Provider::Local => "Motor local (LM Studio, Núcleo IA, Ollama...)",
        }
    }

    /// A stable name for saving the choice.
    pub fn id(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::Gemini => "gemini",
            Provider::DeepSeek => "deepseek",
            Provider::Local => "local",
        }
    }

    pub fn from_id(id: &str) -> Option<Provider> {
        Provider::ALL.into_iter().find(|p| p.id() == id)
    }

    /// A model to start with; the user can type any other the service offers.
    pub fn default_model(self) -> &'static str {
        match self {
            Provider::Anthropic => "claude-sonnet-5-5",
            Provider::Gemini => "gemini-4-hydrogen",
            Provider::DeepSeek => "deepseek-chat",
            Provider::Local => "local-model",
        }
    }

    /// Models offered in the picker. The list is only a convenience: any name the service
    /// accepts can be typed instead.
    pub fn suggested_models(self) -> &'static [&'static str] {
        match self {
            Provider::Anthropic => &["claude-sonnet-5-5", "claude-opus-5-5", "claude-haiku-5-5", "claude-fable-5-2"],
            Provider::Gemini => &["gemini-4-hydrogen", "gemini-4-neon", "gemini-4-carbon", "gemini-4-argon", "gemini-2.5-pro", "gemini-2.5-flash"],
            Provider::DeepSeek => &["deepseek-chat", "deepseek-reasoner"],
            Provider::Local => &[],
        }
    }

    /// Where the person creates an API key.
    pub fn key_page(self) -> &'static str {
        match self {
            Provider::Anthropic => "https://console.anthropic.com/settings/keys",
            Provider::Gemini => "https://aistudio.google.com/apikey",
            Provider::DeepSeek => "https://platform.deepseek.com/api_keys",
            Provider::Local => "",
        }
    }

    /// The environment variable that supplies the key when none is typed.
    pub fn key_variable(self) -> &'static str {
        match self {
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            Provider::Gemini => "GEMINI_API_KEY",
            Provider::DeepSeek => "DEEPSEEK_API_KEY",
            Provider::Local => "LOCAL_AI_API_KEY",
        }
    }

    /// Whether the service takes a reasoning level.
    pub fn supports_reasoning(self) -> bool {
        self != Provider::DeepSeek
    }

    /// Whether the service needs a key (a local engine usually does not).
    pub fn needs_key(self) -> bool {
        self != Provider::Local
    }
}

/// Addresses to start from for a local engine; the person can edit it freely.
pub const LOCAL_PRESETS: [(&str, &str); 4] = [
    ("LM Studio", "http://localhost:1234/v1"),
    ("Ollama", "http://localhost:11434/v1"),
    ("llama.cpp", "http://localhost:8080/v1"),
    ("Núcleo IA", "http://localhost:1234/v1"),
];

/// The address a local engine starts with.
pub const DEFAULT_LOCAL_URL: &str = "http://localhost:1234/v1";

/// A base address that is safe to use: http or https, no spaces, no embedded credentials.
pub fn check_base_url(url: &str) -> Result<String, AiError> {
    let u = url.trim().trim_end_matches('/');
    let ok = (u.starts_with("http://") || u.starts_with("https://")) && u.len() < 200 && !u.chars().any(|c| c.is_whitespace() || c == '@');
    if ok && u.split("://").nth(1).is_some_and(|h| !h.is_empty()) {
        Ok(u.to_string())
    } else {
        Err(AiError("la dirección del motor local debe empezar por http:// o https:// (por ejemplo http://localhost:1234/v1)".into()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub text: String,
    pub attachments: Vec<Attachment>,
}

impl Message {
    pub fn new(role: Role, text: impl Into<String>) -> Message {
        Message { role, text: text.into(), attachments: Vec::new() }
    }
}

/// How much the model should think before it answers. `Auto` sends nothing and leaves the
/// service's own default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Reasoning {
    #[default]
    Auto,
    Low,
    Medium,
    High,
    Max,
}

impl Reasoning {
    pub const ALL: [Reasoning; 5] = [Reasoning::Auto, Reasoning::Low, Reasoning::Medium, Reasoning::High, Reasoning::Max];

    pub fn id(self) -> &'static str {
        match self {
            Reasoning::Auto => "auto",
            Reasoning::Low => "low",
            Reasoning::Medium => "medium",
            Reasoning::High => "high",
            Reasoning::Max => "max",
        }
    }

    pub fn from_id(id: &str) -> Option<Reasoning> {
        Reasoning::ALL.into_iter().find(|r| r.id() == id)
    }
}

/// What kind of file an attachment is, which decides how each service receives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachKind {
    Image,
    Pdf,
    /// Text of any kind (CSV points, LISP, DXF, JSON...): pasted into the message.
    Text,
}

/// A file the person adds to a question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    pub data: Vec<u8>,
}

pub const MAX_ATTACHMENTS: usize = 6;
pub const MAX_BINARY_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 300_000;

impl Attachment {
    /// Read `data` as the file `name`: its kind comes from what the bytes are, then from the name.
    pub fn from_file(name: &str, data: Vec<u8>) -> Result<Attachment, AiError> {
        let name: String = name.chars().filter(|c| !c.is_control()).collect();
        let (mime, kind) = if data.starts_with(b"\x89PNG") {
            ("image/png", AttachKind::Image)
        } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
            ("image/jpeg", AttachKind::Image)
        } else if data.starts_with(b"GIF8") {
            ("image/gif", AttachKind::Image)
        } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
            ("image/webp", AttachKind::Image)
        } else if data.starts_with(b"%PDF") {
            ("application/pdf", AttachKind::Pdf)
        } else {
            ("text/plain", AttachKind::Text)
        };
        let limit = if kind == AttachKind::Text { MAX_TEXT_BYTES } else { MAX_BINARY_BYTES };
        if data.len() > limit {
            return Err(AiError(format!("«{name}» es demasiado grande ({} KB; máximo {} KB)", data.len() / 1024, limit / 1024)));
        }
        if kind == AttachKind::Text && (data.contains(&0) || text_of(&data).is_none()) {
            return Err(AiError(format!("«{name}» no es un tipo de archivo que el asistente pueda leer (para Excel o DWG, expórtalo a CSV o DXF)")));
        }
        Ok(Attachment { name, mime: mime.into(), data })
    }

    pub fn kind(&self) -> AttachKind {
        match self.mime.as_str() {
            "application/pdf" => AttachKind::Pdf,
            m if m.starts_with("image/") => AttachKind::Image,
            _ => AttachKind::Text,
        }
    }
}

/// UTF-8 text, or Windows-1252 text from older programs.
fn text_of(data: &[u8]) -> Option<String> {
    match std::str::from_utf8(data) {
        Ok(t) => Some(t.to_string()),
        Err(_) => {
            let (t, _, bad) = encoding_rs::WINDOWS_1252.decode(data);
            (!bad).then(|| t.into_owned())
        }
    }
}

/// The message text with its text attachments written in, and a note for the binary ones.
fn text_with_files(m: &Message, binary_follows: bool) -> String {
    let mut out = m.text.clone();
    for a in &m.attachments {
        match a.kind() {
            AttachKind::Text => {
                out.push_str(&format!("\n\n[Archivo adjunto: {}]\n```\n{}\n```", a.name, text_of(&a.data).unwrap_or_default().trim_end()));
            }
            _ if !binary_follows => out.push_str(&format!("\n\n[Archivo adjunto en un mensaje anterior: {}]", a.name)),
            _ => {}
        }
    }
    out
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// What one request needs. `Debug` is written by hand so a key never reaches a log.
#[derive(Clone, Default)]
pub struct Settings {
    pub provider: Provider,
    pub model: String,
    pub api_key: String,
    /// Where a local engine listens; unused by the cloud services.
    pub base_url: String,
    /// How much the model thinks; ignored where the service has no such control.
    pub reasoning: Reasoning,
    /// Extra instructions the user adds after the built-in ones.
    pub extra_instructions: String,
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings").field("provider", &self.provider).field("model", &self.model).field("api_key", &"<hidden>").finish_non_exhaustive()
    }
}

/// Why a request failed; the text is safe to show (it never holds the key).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiError(pub String);

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AiError {}

/// The latest messages sent with each request; older ones are dropped to bound cost.
pub const MAX_HISTORY: usize = 24;
/// The most tokens the assistant may answer with.
pub const MAX_TOKENS: u32 = 8192;
const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;

/// The instructions every conversation starts with.
pub fn system_prompt(extra: &str) -> String {
    let mut s = String::from(SYSTEM_PROMPT);
    let extra = extra.trim();
    if !extra.is_empty() {
        s.push_str("\n\nInstrucciones adicionales del usuario:\n");
        s.push_str(extra);
    }
    s
}

const SYSTEM_PROMPT: &str = "\
Eres el asistente de Civil-ING, el módulo de ingeniería civil y topografía de TerraCAD-GIS (una aplicación CAD basada en CADCraft de código abierto escrita en Rust). \
Respondes en el idioma del usuario (por defecto español de Colombia) y trabajas de forma agéntica pero con el usuario al mando: propones, el usuario aprueba y ejecuta. \
Nunca digas que ejecutaste algo: solo propones.\n\
\n\
Para proponer una acción en el dibujo, escribe un bloque de código con la etiqueta cadcraft que contenga un objeto JSON {\"command\": \"id\", \"params\": {...}} o una lista de ellos. \
TerraCAD-GIS mostrará cada comando y el usuario decidirá si lo ejecuta. Comandos de Civil-ING:\n\
- civil.points.import {path, columns?: \"PNEZD\"|\"PENZD\"|\"PXYZD\"|\"XYZ\"..., delimiter?, layer?, labels?: bool, text_height?, decimals?} — importa puntos de CSV, TXT, XLSX, DXF o DWG. Letras de columnas: P punto, N norte, E este, X, Y, Z cota, D descripción, - ignorar.\n\
- civil.points.export {path, delimiter?} — guarda en CSV los puntos (objetos POINT) del dibujo.\n\
- civil.tin {max_edge?, layer?} — crea la triangulación (TIN) con caras 3D a partir de los puntos con cota.\n\
- civil.points.radiate {path, station: {id?, x, y, z?, hi?, backsight_azimuth?}, columns?: \"PV ANGH ANGV DINC DHOR DIFH HAP DESC\", angles?: \"deg\"|\"dms\"|\"grad\", order?, join?: \"polyline\"|\"polyline3d\", labels?} — calcula puntos desde observaciones de estación total (radiaciones).\n\
- civil.points.join {source_layer?, kind?: \"polyline3d\"|\"polyline\", close?} — une los puntos del dibujo con una polilínea.\n\
- civil.parcel {handle? (polilínea cerrada), start?, direction?: \"cw\"|\"ccw\", naming?: \"numbers\"|\"letters\", decimals?, at?: [x,y], language?: \"es\"|\"en\"} — dibuja el cuadro de construcción (rumbo, distancia, ángulo interior, coordenadas, superficie) y el cuadro de curvas.\n\
- civil.earthwork {handle?, pvis: \"0,100; 120,104,40; 300,101\" (estación,cota[,curva vertical]), half_width?, crossfall?, interval?, factor?, path?} — áreas de corte y relleno por sección, volúmenes por áreas medias y curva masa.\n\
- civil.sections {handle?, pvis?, half_width?, interval?, columns?, vscale?} — dibuja las secciones transversales con la plantilla de la rasante.\n\
- civil.profile también acepta pvis para dibujar la rasante, las curvas verticales y los espesores de corte (C) y relleno (T).\n\
- civil.platform {handle? (polilínea cerrada), elevation?, cut_slope?, fill_slope?, bench_height?, berm_width?, method?: \"prismoidal\"|\"average\", interval?, step?} — plataforma con taludes (corte y terraplén), bermas, línea de talud, línea cero y volúmenes por secciones.\n\
- civil.text.arc {text, center?: [x,y], radius?, handle? (arco o círculo), angle?, height?, reverse?} — escribe texto siguiendo un arco. civil.text.number {handles?, prefix?, start?, step?, digits?, order?} numera textos; civil.text.swap intercambia dos textos; civil.text.space {spacing?, axis?} reespacia; civil.text.height {scale, mm?} pone altura real en mm de papel; civil.text.export / civil.text.import {path} pasan textos a/desde CSV.\n\
- civil.detail.list lista los bloques (detalles); civil.detail.insert {name, at: [x,y], width? | paper_mm? + scale?} inserta un detalle con escala automática.\n\
- civil.contours {interval, base?, major_every?, max_edge?, break_layer?, break_step?, minor_color?, major_color?, labels?, text_height?} — curvas de nivel menores y mayores; break_layer indica la capa de líneas de ruptura.\n\
- civil.stations {handle?, interval?, start?, text_height?} — estaciones (abscisas, p. ej. 1+250.00) sobre un eje (polilínea) por su handle o la selección.\n\
- civil.profile {handle?, interval?, step?, origin?: [x,y], vscale?, max_edge?} — perfil longitudinal del terreno a lo largo de un eje.\n\
Otros comandos de TerraCAD-GIS (line, pline, circle, text, layer, zoom...) también pueden proponerse con su JSON.\n\
\n\
Para tareas que necesitan lógica, repetición o comandos propios, puedes proponer una rutina AutoLISP en un bloque con la etiqueta lisp (por ejemplo (defun c:NOMBRE () ...) o una secuencia de (command ...)); \
TerraCAD-GIS la guarda como un paso del procedimiento. Los bloques cadcraft y lisp de una respuesta se pueden ejecutar todos, probar sobre una copia del dibujo (que se deshace) y guardar como procedimiento con un botón en la barra del asistente; \
tras una prueba el usuario te devolverá un informe por paso: si hay errores, responde con la versión corregida completa. \
Si el usuario envía una grabación de macro (líneas `command`, `point`, `keyword`, `text`, `pick`, `run`), explica qué intentó hacer y generaliza: sustituye lo específico del dibujo por parámetros y avisa cuáles. \
Un complemento (.tcgplugin) es un archivo JSON {format: 1, id, name, version, menu, items: [{label, procedure}], toolbar: [ids], procedures: [{id, name, description, steps: [{command, params}], icon, toolbar}], lisp: [{name, code}]} que añade un menú y una barra sin compilar; puedes redactarlo si te lo piden.\n\
\n\
Si el usuario pide mejorar la aplicación, propón cambios como código Rust o un diff unificado en bloques de código con la etiqueta del lenguaje, \
explica brevemente por qué, indica en qué archivo o crate iría (el proyecto es un workspace de Rust: crates/civil es Civil-ING, crates/engine los comandos, crates/ui-egui la interfaz) \
y qué pruebas lo cubrirían. Sé concreto y honesto sobre lo que no sabes o no puedes verificar. \
Cuando el usuario te pida modificar la propia aplicación, responde con un parche en formato diff unificado (git) en un bloque con la etiqueta diff: rutas relativas a la raíz del proyecto (a/crates/... b/crates/...), cambios mínimos y pruebas incluidas. \
Debajo del bloque, el usuario verá el botón «Aplicar al código fuente»: se guarda como un commit propio (reversible) y la aplicación le pedirá recompilar y reiniciar para activarlo, también entre sesiones. \
Cada parche debe aplicarse sobre el código tal como te lo enviamos; si dudas del contenido actual de un archivo, pídelo antes de proponer el parche. Nunca incluyas claves, tokens ni datos personales en el código. \
La conversación se conserva entre sesiones: puedes retomar lo anterior. Si una respuesta tuya se corta, la aplicación te pedirá continuar. \
No inventes normas ni valores de diseño: si una cifra depende de una norma (INVIAS, RAS, NSR-10), nómbrala y pide al usuario que la verifique.";

/// A request ready to send.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub url: String,
    /// `(name, value)`; one of them holds the API key.
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

fn clip(history: &[Message]) -> &[Message] {
    &history[history.len().saturating_sub(MAX_HISTORY)..]
}

/// The request for `history` (which must end with the user's message).
pub fn build_request(s: &Settings, history: &[Message]) -> Result<Request, AiError> {
    if s.provider.needs_key() && s.api_key.trim().is_empty() {
        return Err(AiError(format!(
            "falta la clave de API de {} (escríbela en Configuración o defina {})",
            s.provider.label(),
            s.provider.key_variable()
        )));
    }
    let model = s.model.trim();
    let model = if model.is_empty() { s.provider.default_model() } else { model };
    // Local engines name models like `qwen/qwen3-8b`; a slash is fine there, a `..` never is.
    let slash = s.provider == Provider::Local;
    if model.contains("..") || !model.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':') || slash && c == '/') {
        return Err(AiError("el nombre del modelo solo puede tener letras, números, punto, guion y guion bajo".into()));
    }
    if history.last().is_none_or(|m| m.role != Role::User) {
        return Err(AiError("el último mensaje debe ser del usuario".into()));
    }
    let system = system_prompt(&s.extra_instructions);
    let key = s.api_key.trim().to_string();
    let history = clip(history);
    check_attachments(s.provider, history)?;
    let reasoning = if s.provider.supports_reasoning() { s.reasoning } else { Reasoning::Auto };
    // Thinking tokens count against the limit, so a reasoning request needs room for them.
    let max_tokens = if reasoning == Reasoning::Auto { MAX_TOKENS } else { MAX_TOKENS_REASONING };
    Ok(match s.provider {
        Provider::Anthropic => {
            let mut body = json!({
                "model": model,
                "max_tokens": max_tokens,
                "system": system,
                "messages": history.iter().enumerate().map(|(i, m)| json!({
                    "role": if m.role == Role::User { "user" } else { "assistant" },
                    "content": anthropic_content(m, i + 1 == history.len()),
                })).collect::<Vec<_>>(),
            });
            if reasoning != Reasoning::Auto {
                // Adaptive thinking is on by itself in current models; effort says how much.
                body["output_config"] = json!({ "effort": reasoning.id() });
            }
            Request {
                url: "https://api.anthropic.com/v1/messages".into(),
                headers: vec![
                    ("x-api-key".into(), key),
                    ("anthropic-version".into(), "2023-06-01".into()),
                    ("content-type".into(), "application/json".into()),
                ],
                body,
            }
        }
        Provider::Gemini => {
            let mut config = json!({ "maxOutputTokens": max_tokens });
            if let Some(t) = gemini_thinking(model, reasoning) {
                config["thinkingConfig"] = t;
            }
            Request {
                url: format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"),
                headers: vec![("x-goog-api-key".into(), key), ("content-type".into(), "application/json".into())],
                body: json!({
                    "systemInstruction": { "parts": [{ "text": system }] },
                    "contents": history.iter().enumerate().map(|(i, m)| json!({
                        "role": if m.role == Role::User { "user" } else { "model" },
                        "parts": gemini_parts(m, i + 1 == history.len()),
                    })).collect::<Vec<_>>(),
                    "generationConfig": config,
                }),
            }
        }
        // DeepSeek speaks the OpenAI chat-completions dialect.
        Provider::DeepSeek => {
            openai_request("https://api.deepseek.com/chat/completions".into(), Some(&key), model, &system, history, Reasoning::Auto, max_tokens)
        }
        Provider::Local => {
            let base = check_base_url(&s.base_url)?;
            openai_request(
                format!("{base}/chat/completions"),
                (!key.is_empty()).then_some(key.as_str()),
                model,
                &system,
                history,
                reasoning,
                max_tokens,
            )
        }
    })
}

/// Room for the answer when the model is asked to think.
pub const MAX_TOKENS_REASONING: u32 = 16_000;

/// Gemini's thinking control: a level for Gemini 3 and later, a token budget for 2.5.
/// The two are never sent together. Other models get nothing.
fn gemini_thinking(model: &str, r: Reasoning) -> Option<Value> {
    if r == Reasoning::Auto {
        return None;
    }
    let m = model.to_ascii_lowercase();
    if m.contains("gemini-2.5") {
        let budget = match r {
            Reasoning::Low => 1024,
            Reasoning::Medium => 4096,
            Reasoning::High => 12_288,
            _ => 24_576,
        };
        Some(json!({ "thinkingBudget": budget }))
    } else if m.contains("gemini-3") || m.contains("gemini-4") {
        let level = match r {
            Reasoning::Low => "low",
            Reasoning::Medium => "medium",
            _ => "high",
        };
        Some(json!({ "thinkingLevel": level }))
    } else {
        None
    }
}

/// Refuse an attachment the service cannot take, saying which and why.
fn check_attachments(p: Provider, history: &[Message]) -> Result<(), AiError> {
    let Some(last) = history.last() else { return Ok(()) };
    if last.attachments.len() > MAX_ATTACHMENTS {
        return Err(AiError(format!("hay demasiados archivos adjuntos (máximo {MAX_ATTACHMENTS})")));
    }
    for a in &last.attachments {
        let ok = match a.kind() {
            AttachKind::Text => true,
            AttachKind::Image => p != Provider::DeepSeek,
            AttachKind::Pdf => matches!(p, Provider::Anthropic | Provider::Gemini),
        };
        if !ok {
            return Err(AiError(format!(
                "{} no admite «{}»: {}",
                p.label(),
                a.name,
                if a.kind() == AttachKind::Pdf {
                    "los PDF solo los leen Anthropic y Gemini; copia el texto o cambia de servicio"
                } else {
                    "este servicio solo lee texto; cambia de servicio o adjunta texto"
                }
            )));
        }
    }
    Ok(())
}

fn anthropic_content(m: &Message, last: bool) -> Value {
    let text = text_with_files(m, last);
    if !last || m.attachments.iter().all(|a| a.kind() == AttachKind::Text) {
        return json!(text);
    }
    let mut parts: Vec<Value> = m
        .attachments
        .iter()
        .filter(|a| a.kind() != AttachKind::Text)
        .map(|a| {
            let source = json!({ "type": "base64", "media_type": a.mime, "data": base64(&a.data) });
            if a.kind() == AttachKind::Pdf { json!({ "type": "document", "source": source }) } else { json!({ "type": "image", "source": source }) }
        })
        .collect();
    parts.push(json!({ "type": "text", "text": text }));
    Value::Array(parts)
}

fn gemini_parts(m: &Message, last: bool) -> Value {
    let mut parts = vec![json!({ "text": text_with_files(m, last) })];
    if last {
        parts.extend(
            m.attachments
                .iter()
                .filter(|a| a.kind() != AttachKind::Text)
                .map(|a| json!({ "inline_data": { "mime_type": a.mime, "data": base64(&a.data) } })),
        );
    }
    Value::Array(parts)
}

fn openai_content(m: &Message, last: bool) -> Value {
    let text = text_with_files(m, last);
    if !last || m.attachments.iter().all(|a| a.kind() == AttachKind::Text) {
        return json!(text);
    }
    let mut parts = vec![json!({ "type": "text", "text": text })];
    parts.extend(
        m.attachments
            .iter()
            .filter(|a| a.kind() == AttachKind::Image)
            .map(|a| json!({ "type": "image_url", "image_url": { "url": format!("data:{};base64,{}", a.mime, base64(&a.data)) } })),
    );
    Value::Array(parts)
}

fn openai_request(url: String, key: Option<&str>, model: &str, system: &str, history: &[Message], reasoning: Reasoning, max_tokens: u32) -> Request {
    let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
    if let Some(k) = key {
        headers.push(("authorization".into(), format!("Bearer {k}")));
    }
    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": false,
        "messages": std::iter::once(json!({ "role": "system", "content": system }))
            .chain(history.iter().enumerate().map(|(i, m)| json!({
                "role": if m.role == Role::User { "user" } else { "assistant" },
                "content": openai_content(m, i + 1 == history.len()),
            })))
            .collect::<Vec<_>>(),
    });
    if reasoning != Reasoning::Auto {
        body["reasoning_effort"] = json!(if reasoning == Reasoning::Max { "high" } else { reasoning.id() });
    }
    Request { url, headers, body }
}

/// The request that lists the models the person's key (or local engine) can use.
pub fn models_request(s: &Settings) -> Result<Request, AiError> {
    let key = s.api_key.trim().to_string();
    if s.provider.needs_key() && key.is_empty() {
        return Err(AiError(format!("escribe primero la clave de API de {} para consultar sus modelos", s.provider.label())));
    }
    let json_header = ("accept".to_string(), "application/json".to_string());
    Ok(match s.provider {
        Provider::Anthropic => Request {
            url: "https://api.anthropic.com/v1/models?limit=100".into(),
            headers: vec![("x-api-key".into(), key), ("anthropic-version".into(), "2023-06-01".into()), json_header],
            body: Value::Null,
        },
        Provider::Gemini => Request {
            url: "https://generativelanguage.googleapis.com/v1beta/models?pageSize=200".into(),
            headers: vec![("x-goog-api-key".into(), key), json_header],
            body: Value::Null,
        },
        Provider::DeepSeek => Request {
            url: "https://api.deepseek.com/models".into(),
            headers: vec![("authorization".into(), format!("Bearer {key}")), json_header],
            body: Value::Null,
        },
        Provider::Local => {
            let base = check_base_url(&s.base_url)?;
            let mut headers = vec![json_header];
            if !key.is_empty() {
                headers.push(("authorization".into(), format!("Bearer {key}")));
            }
            Request { url: format!("{base}/models"), headers, body: Value::Null }
        }
    })
}

/// The model names in a model-list reply, in the order the service gave them. For Gemini only
/// the models that can answer a chat (`generateContent`) are kept.
pub fn parse_model_list(provider: Provider, body: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let mut names: Vec<String> = if provider == Provider::Gemini {
        v.get("models")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter(|m| {
                        m.get("supportedGenerationMethods")
                            .and_then(Value::as_array)
                            .is_some_and(|x| x.iter().any(|g| g.as_str() == Some("generateContent")))
                    })
                    .filter_map(|m| m.get("name").and_then(Value::as_str))
                    .map(|n| n.trim_start_matches("models/").to_string())
                    .filter(|n| n.starts_with("gemini"))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        v.get("data")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|m| m.get("id").and_then(Value::as_str)).map(str::to_string).collect())
            .unwrap_or_default()
    };
    let mut seen = std::collections::HashSet::new();
    names.retain(|n| seen.insert(n.clone()));
    names
}

/// Ask the service which models it offers. Blocks briefly.
#[cfg(not(target_arch = "wasm32"))]
pub fn list_models(s: &Settings) -> Result<Vec<String>, AiError> {
    use std::io::Read;
    use std::time::Duration;
    let req = models_request(s)?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(8))).http_status_as_error(false).build().into();
    let mut r = agent.get(&req.url);
    for (k, v) in &req.headers {
        r = r.header(k.as_str(), v.as_str());
    }
    let resp = r.call().map_err(|e| AiError(network_error(&e.to_string())))?;
    let mut text = String::new();
    resp.into_body().into_reader().take(2 * 1024 * 1024).read_to_string(&mut text).map_err(|e| AiError(network_error(&e.to_string())))?;
    if let Some(msg) = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string))
    {
        return Err(AiError(msg));
    }
    let names = parse_model_list(s.provider, &text);
    if names.is_empty() {
        Err(AiError(if s.provider == Provider::Local {
            "el motor no listó modelos (¿está iniciado el servidor y hay un modelo cargado?)".into()
        } else {
            "el servicio no devolvió modelos".into()
        }))
    } else {
        Ok(names)
    }
}

#[cfg(target_arch = "wasm32")]
pub fn list_models(_: &Settings) -> Result<Vec<String>, AiError> {
    Err(AiError("no disponible en la versión web".into()))
}

/// What a service answered: the text, and whether the service stopped because it ran out of tokens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub text: String,
    /// The reply was cut at the token limit (the rest can be asked for with [`continue_prompt`]).
    pub truncated: bool,
}

/// The reply text out of a service's JSON answer, or the error the service reported.
pub fn parse_reply(provider: Provider, body: &str) -> Result<String, AiError> {
    parse_answer(provider, body).map(|a| a.text)
}

/// Like [`parse_reply`], and says whether the reply was cut at the token limit.
pub fn parse_answer(provider: Provider, body: &str) -> Result<Answer, AiError> {
    let v: Value = serde_json::from_str(body).map_err(|_| AiError("la respuesta del servicio no es JSON".into()))?;
    if let Some(msg) = v.pointer("/error/message").and_then(Value::as_str) {
        return Err(AiError(msg.to_string()));
    }
    let text = match provider {
        Provider::Anthropic => v.get("content").and_then(Value::as_array).map(|parts| {
            parts
                .iter()
                .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        }),
        Provider::Gemini => v.pointer("/candidates/0/content/parts").and_then(Value::as_array).map(|parts| {
            parts
                .iter()
                .filter(|p| p.get("thought").and_then(Value::as_bool) != Some(true))
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        }),
        Provider::DeepSeek | Provider::Local => v.pointer("/choices/0/message/content").and_then(Value::as_str).map(str::to_string),
    };
    let truncated = match provider {
        Provider::Anthropic => v.get("stop_reason").and_then(Value::as_str) == Some("max_tokens"),
        Provider::Gemini => v.pointer("/candidates/0/finishReason").and_then(Value::as_str) == Some("MAX_TOKENS"),
        Provider::DeepSeek | Provider::Local => v.pointer("/choices/0/finish_reason").and_then(Value::as_str) == Some("length"),
    };
    match text {
        Some(text) if !text.trim().is_empty() => Ok(Answer { text, truncated }),
        _ => Err(AiError("el servicio no devolvió texto (¿bloqueó la respuesta o se agotó el límite?)".into())),
    }
}

/// How many times a cut reply is continued by itself before the person is asked.
pub const MAX_CONTINUATIONS: usize = 3;

/// The message that asks the assistant to carry on where a cut reply stopped.
pub fn continue_prompt() -> &'static str {
    "Tu respuesta anterior se cortó por el límite de longitud. Continúa exactamente donde quedaste, sin repetir lo ya escrito y sin saludar; si estabas dentro de un bloque de código, sigue dentro del mismo bloque."
}

/// One request and its raw answer: `(HTTP status, body)`.
#[cfg(not(target_arch = "wasm32"))]
fn post(req: &Request, timeout_secs: u64) -> Result<(u16, String), AiError> {
    use std::io::Read;
    use std::time::Duration;
    let agent: ureq::Agent =
        ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(timeout_secs))).http_status_as_error(false).build().into();
    let mut r = agent.post(&req.url);
    for (k, v) in &req.headers {
        r = r.header(k.as_str(), v.as_str());
    }
    let resp = r.send(req.body.to_string()).map_err(|e| AiError(network_error(&e.to_string())))?;
    let status = resp.status().as_u16();
    let mut text = String::new();
    resp.into_body().into_reader().take(MAX_REPLY_BYTES as u64).read_to_string(&mut text).map_err(|e| AiError(network_error(&e.to_string())))?;
    Ok((status, text))
}

/// The error to show for an unreadable answer with HTTP `status`.
fn status_error(status: u16, e: AiError) -> AiError {
    if !(200..300).contains(&status) && e.0.starts_with("la respuesta") {
        AiError(format!("el servicio respondió con el estado HTTP {status}"))
    } else {
        e
    }
}

/// Send `history` and return the assistant's reply. Blocks: call it from a worker thread. A reply cut at the
/// token limit is continued up to [`MAX_CONTINUATIONS`] times and joined, so long answers arrive whole.
#[cfg(not(target_arch = "wasm32"))]
pub fn chat_answer(s: &Settings, history: &[Message]) -> Result<Answer, AiError> {
    let mut messages: Vec<Message> = history.to_vec();
    let mut full = String::new();
    for round in 0..=MAX_CONTINUATIONS {
        let req = build_request(s, &messages)?;
        let (status, text) = post(&req, 120)?;
        let answer = parse_answer(s.provider, &text).map_err(|e| status_error(status, e))?;
        full.push_str(&answer.text);
        if !answer.truncated || round == MAX_CONTINUATIONS {
            return Ok(Answer { text: full, truncated: answer.truncated });
        }
        messages.push(Message::new(Role::Assistant, answer.text));
        messages.push(Message::new(Role::User, continue_prompt()));
    }
    Ok(Answer { text: full, truncated: true })
}

#[cfg(target_arch = "wasm32")]
pub fn chat_answer(_: &Settings, _: &[Message]) -> Result<Answer, AiError> {
    Err(AiError("el asistente de IA no está disponible en la versión web".into()))
}

/// Send `history` and return the assistant's reply text. Blocks: call it from a worker thread.
pub fn chat(s: &Settings, history: &[Message]) -> Result<String, AiError> {
    chat_answer(s, history).map(|a| a.text)
}

/// What a connection test found, step by step.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TestReport {
    /// The service answered a real request with the key and model in use.
    pub ok: bool,
    /// One line per step, in order (each starts with ✔ or ✘).
    pub lines: Vec<String>,
}

/// Put the token limit of a built request at `n`.
fn limit_tokens(req: &mut Request, provider: Provider, n: u32) {
    match provider {
        Provider::Gemini => req.body["generationConfig"]["maxOutputTokens"] = json!(n),
        _ => req.body["max_tokens"] = json!(n),
    }
}

/// Check the connection: the address and key (by listing the models) and the model (by asking for one word).
/// Blocks briefly; the key never appears in the report.
#[cfg(not(target_arch = "wasm32"))]
pub fn test_connection(s: &Settings) -> TestReport {
    use std::time::Instant;
    let mut report = TestReport::default();
    let model = if s.model.trim().is_empty() { s.provider.default_model().to_string() } else { s.model.trim().to_string() };
    report.lines.push(format!("Servicio: {} · modelo: {model}", s.provider.label()));
    if s.provider == Provider::Local {
        match check_base_url(&s.base_url) {
            Ok(u) => report.lines.push(format!("Dirección: {u}")),
            Err(e) => {
                report.lines.push(format!("✘ {e}"));
                return report;
            }
        }
    }
    if s.provider.needs_key() && s.api_key.trim().is_empty() {
        report.lines.push(format!(
            "✘ Falta la clave de API de {} (escríbela en Configuración o defina {}).",
            s.provider.label(),
            s.provider.key_variable()
        ));
        return report;
    }
    let started = Instant::now();
    let listed = match list_models(s) {
        Ok(names) => {
            report.lines.push(format!("✔ Conexión y credenciales válidas: {} modelos en {} ms", names.len(), started.elapsed().as_millis()));
            Some(names)
        }
        Err(e) => {
            report.lines.push(format!("✘ No se pudieron listar los modelos: {e}"));
            None
        }
    };
    if let Some(names) = &listed
        && !names.iter().any(|n| n == &model)
        && s.provider != Provider::Local
    {
        report.lines.push(format!("⚠ El modelo «{model}» no está en la lista del servicio; elige otro en «Modelos disponibles»."));
    }
    // A tiny real request: it proves the key may use the model, and measures the answer time.
    let mut probe = s.clone();
    probe.reasoning = Reasoning::Auto;
    probe.extra_instructions.clear();
    let history = [Message::new(Role::User, "Responde solo con la palabra: OK")];
    let started = Instant::now();
    match build_request(&probe, &history).and_then(|mut req| {
        limit_tokens(&mut req, probe.provider, 64);
        post(&req, 30)
    }) {
        Ok((status, body)) => match parse_answer(s.provider, &body) {
            Ok(a) => {
                report.ok = true;
                report.lines.push(format!(
                    "✔ El modelo respondió en {} ms: «{}»",
                    started.elapsed().as_millis(),
                    a.text.trim().chars().take(40).collect::<String>()
                ));
            }
            Err(e) if (200..300).contains(&status) && e.0.starts_with("el servicio no devolvió texto") => {
                // Reasoning models may spend the whole small limit thinking: the service still accepted the request.
                report.ok = true;
                report.lines.push(format!(
                    "✔ El modelo aceptó la solicitud ({} ms), aunque no devolvió texto con el límite de prueba.",
                    started.elapsed().as_millis()
                ));
            }
            Err(e) => report.lines.push(format!("✘ {}", status_error(status, e))),
        },
        Err(e) => report.lines.push(format!("✘ {e}")),
    }
    report
}

#[cfg(target_arch = "wasm32")]
pub fn test_connection(_: &Settings) -> TestReport {
    TestReport { ok: false, lines: vec!["✘ no disponible en la versión web".into()] }
}

/// A transport error as text; the request (and its key) is never part of it.
fn network_error(e: &str) -> String {
    format!("no se pudo contactar al servicio de IA: {e}")
}

/// A command the assistant proposed.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub command: String,
    pub params: Value,
}

/// A fenced code block of a reply: its language tag and text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeBlock {
    pub lang: String,
    pub text: String,
}

/// The fenced code blocks of `reply`, in order.
pub fn code_blocks(reply: &str) -> Vec<CodeBlock> {
    let mut out = Vec::new();
    let mut cur: Option<(String, String)> = None;
    for line in reply.lines() {
        let t = line.trim_start();
        if let Some(tag) = t.strip_prefix("```") {
            match cur.take() {
                Some((lang, text)) => out.push(CodeBlock { lang, text }),
                None => cur = Some((tag.trim().to_lowercase(), String::new())),
            }
        } else if let Some((_, text)) = cur.as_mut() {
            text.push_str(line);
            text.push('\n');
        }
    }
    out
}

/// The commands in the reply's ```cadcraft blocks. A block that is not valid JSON, or names no
/// command, is skipped (it is still shown as text).
pub fn proposals(reply: &str) -> Vec<Proposal> {
    code_blocks(reply).into_iter().filter(|b| b.lang == "cadcraft").flat_map(|b| proposals_of(&b.text)).collect()
}

/// The commands in the text of one ```cadcraft block: a JSON object or a list of them.
pub fn proposals_of(block: &str) -> Vec<Proposal> {
    let mut out = Vec::new();
    let Ok(v) = serde_json::from_str::<Value>(block) else { return out };
    let items = match v {
        Value::Array(a) => a,
        o => vec![o],
    };
    for it in items {
        let Some(c) = it.get("command").and_then(Value::as_str) else { continue };
        if c.is_empty() || c.len() > 64 || !c.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
            continue;
        }
        out.push(Proposal { command: c.to_string(), params: it.get("params").cloned().unwrap_or_else(|| json!({})) });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_cut_at_the_token_limit_is_flagged_for_every_service() {
        let cut = [
            (Provider::Anthropic, r#"{"content":[{"type":"text","text":"abc"}],"stop_reason":"max_tokens"}"#),
            (Provider::Gemini, r#"{"candidates":[{"content":{"parts":[{"text":"abc"}]},"finishReason":"MAX_TOKENS"}]}"#),
            (Provider::DeepSeek, r#"{"choices":[{"message":{"content":"abc"},"finish_reason":"length"}]}"#),
            (Provider::Local, r#"{"choices":[{"message":{"content":"abc"},"finish_reason":"length"}]}"#),
        ];
        for (p, body) in cut {
            let a = parse_answer(p, body).unwrap();
            assert_eq!((a.text.as_str(), a.truncated), ("abc", true), "{p:?}");
        }
        let whole = parse_answer(Provider::Anthropic, r#"{"content":[{"type":"text","text":"abc"}],"stop_reason":"end_turn"}"#).unwrap();
        assert!(!whole.truncated);
        assert_eq!(parse_reply(Provider::Anthropic, r#"{"content":[{"type":"text","text":"abc"}]}"#).unwrap(), "abc");
    }

    #[test]
    fn the_whole_conversation_goes_with_every_request_and_the_probe_is_small() {
        let history = vec![Message::new(Role::User, "primera"), Message::new(Role::Assistant, "respuesta"), Message::new(Role::User, "segunda")];
        let mut s = settings(Provider::Anthropic);
        s.api_key = "k".into();
        let mut req = build_request(&s, &history).unwrap();
        assert_eq!(req.body["messages"].as_array().unwrap().len(), 3);
        assert_eq!(req.body["max_tokens"], MAX_TOKENS);
        limit_tokens(&mut req, Provider::Anthropic, 64);
        assert_eq!(req.body["max_tokens"], 64);
        let mut g = settings(Provider::Gemini);
        g.api_key = "k".into();
        let mut req = build_request(&g, &history).unwrap();
        limit_tokens(&mut req, Provider::Gemini, 64);
        assert_eq!(req.body["generationConfig"]["maxOutputTokens"], 64);
        assert!(continue_prompt().contains("Continúa"));
    }

    #[test]
    fn testing_a_local_engine_with_a_bad_address_stops_at_the_address() {
        let mut s = settings(Provider::Local);
        s.base_url = "ftp://nada".into();
        let r = test_connection(&s);
        assert!(!r.ok);
        assert!(r.lines.iter().any(|l| l.starts_with('✘')));
        assert!(r.lines.iter().all(|l| !l.contains("ftp://nada") || l.starts_with('✘')));
    }

    fn settings(p: Provider) -> Settings {
        Settings {
            provider: p,
            model: String::new(),
            api_key: "sk-test-123".into(),
            base_url: "http://localhost:1234/v1".into(),
            reasoning: Reasoning::Auto,
            extra_instructions: String::new(),
        }
    }

    fn ask(text: &str) -> Vec<Message> {
        vec![Message::new(Role::User, text)]
    }

    #[test]
    fn anthropic_request_carries_the_key_in_a_header_never_in_the_body() {
        let r = build_request(&settings(Provider::Anthropic), &ask("hola")).unwrap();
        assert!(r.headers.iter().any(|(k, v)| k == "x-api-key" && v == "sk-test-123"));
        assert!(!r.body.to_string().contains("sk-test-123"));
        assert_eq!(r.body["messages"][0]["content"], "hola");
        assert_eq!(r.body["model"], Provider::Anthropic.default_model());
    }

    #[test]
    fn gemini_request_names_the_model_in_the_url_and_maps_roles() {
        let mut s = settings(Provider::Gemini);
        s.model = "gemini-2.5-pro".into();
        let h = vec![Message::new(Role::User, "a"), Message::new(Role::Assistant, "b"), Message::new(Role::User, "c")];
        let r = build_request(&s, &h).unwrap();
        assert!(r.url.ends_with("/models/gemini-2.5-pro:generateContent"));
        assert_eq!(r.body["contents"][1]["role"], "model");
        assert!(!r.url.contains("sk-test-123"), "the key is never in the URL");
    }

    #[test]
    fn deepseek_request_uses_a_bearer_header_and_a_system_message() {
        let mut s = settings(Provider::DeepSeek);
        s.model = "deepseek-reasoner".into();
        let r = build_request(&s, &ask("hola")).unwrap();
        assert_eq!(r.url, "https://api.deepseek.com/chat/completions");
        assert!(r.headers.iter().any(|(k, v)| k == "authorization" && v == "Bearer sk-test-123"));
        assert!(!r.body.to_string().contains("sk-test-123"));
        assert_eq!(r.body["model"], "deepseek-reasoner");
        assert_eq!(r.body["messages"][0]["role"], "system");
        assert_eq!(r.body["messages"][1]["content"], "hola");
        let ok = r#"{"choices":[{"message":{"role":"assistant","content":"listo"}}]}"#;
        assert_eq!(parse_reply(Provider::DeepSeek, ok).unwrap(), "listo");
        assert_eq!(parse_reply(Provider::DeepSeek, r#"{"error":{"message":"Authentication Fails"}}"#).unwrap_err().0, "Authentication Fails");
    }

    #[test]
    fn a_local_engine_needs_no_key_and_validates_its_address() {
        let mut s = settings(Provider::Local);
        s.api_key.clear();
        s.model = "qwen/qwen3-8b".into();
        let r = build_request(&s, &ask("hola")).unwrap();
        assert_eq!(r.url, "http://localhost:1234/v1/chat/completions");
        assert!(r.headers.iter().all(|(k, _)| k != "authorization"));
        assert_eq!(r.body["model"], "qwen/qwen3-8b");
        s.api_key = "k".into();
        s.base_url = "http://192.168.1.20:8080/v1/".into();
        let r = build_request(&s, &ask("hola")).unwrap();
        assert_eq!(r.url, "http://192.168.1.20:8080/v1/chat/completions");
        assert!(r.headers.iter().any(|(k, v)| k == "authorization" && v == "Bearer k"));
        for bad in ["", "localhost:1234", "ftp://x", "http://user:pw@host/v1", "http://a b/v1", "http://"] {
            s.base_url = bad.into();
            assert!(build_request(&s, &ask("x")).is_err(), "{bad}");
        }
        s.base_url = DEFAULT_LOCAL_URL.into();
        s.model = "../x".into();
        assert!(build_request(&s, &ask("x")).is_err());
        // Cloud services never accept a slash in the model name.
        let mut g = settings(Provider::Anthropic);
        g.model = "a/b".into();
        assert!(build_request(&g, &ask("x")).is_err());
    }

    #[test]
    fn model_lists_are_read_for_every_service() {
        assert_eq!(parse_model_list(Provider::Local, r#"{"data":[{"id":"b"},{"id":"a"},{"id":"b"}]}"#), vec!["b", "a"]);
        assert!(parse_model_list(Provider::Local, "no json").is_empty());
        let gem = r#"{"models":[
            {"name":"models/gemini-4-hydrogen","supportedGenerationMethods":["generateContent"]},
            {"name":"models/gemini-4-neon","supportedGenerationMethods":["generateContent","countTokens"]},
            {"name":"models/text-embedding-004","supportedGenerationMethods":["embedContent"]},
            {"name":"models/gemini-embedding-001","supportedGenerationMethods":["embedContent"]}]}"#;
        assert_eq!(parse_model_list(Provider::Gemini, gem), vec!["gemini-4-hydrogen", "gemini-4-neon"]);
        let r = models_request(&settings(Provider::Gemini)).unwrap();
        assert!(r.url.contains("/v1beta/models"));
        assert!(r.headers.iter().any(|(k, _)| k == "x-goog-api-key"));
        let mut empty = settings(Provider::Anthropic);
        empty.api_key.clear();
        assert!(models_request(&empty).is_err());
        let mut local = settings(Provider::Local);
        local.api_key.clear();
        assert_eq!(models_request(&local).unwrap().url, "http://localhost:1234/v1/models");
    }

    #[test]
    fn reasoning_is_sent_the_way_each_service_takes_it() {
        let mut s = settings(Provider::Anthropic);
        assert!(build_request(&s, &ask("x")).unwrap().body.get("output_config").is_none());
        s.reasoning = Reasoning::High;
        let r = build_request(&s, &ask("x")).unwrap();
        assert_eq!(r.body["output_config"]["effort"], "high");
        assert_eq!(r.body["max_tokens"], MAX_TOKENS_REASONING);
        assert!(r.body.get("thinking").is_none());

        let mut g = settings(Provider::Gemini);
        g.reasoning = Reasoning::Low;
        g.model = "gemini-4-neon".into();
        assert_eq!(build_request(&g, &ask("x")).unwrap().body["generationConfig"]["thinkingConfig"], json!({ "thinkingLevel": "low" }));
        g.model = "gemini-2.5-pro".into();
        g.reasoning = Reasoning::Medium;
        let body = build_request(&g, &ask("x")).unwrap().body;
        assert_eq!(body["generationConfig"]["thinkingConfig"], json!({ "thinkingBudget": 4096 }));
        g.model = "gemini-1.5-flash".into();
        assert!(build_request(&g, &ask("x")).unwrap().body["generationConfig"].get("thinkingConfig").is_none());

        let mut l = settings(Provider::Local);
        l.reasoning = Reasoning::Max;
        assert_eq!(build_request(&l, &ask("x")).unwrap().body["reasoning_effort"], "high");
        let mut d = settings(Provider::DeepSeek);
        d.reasoning = Reasoning::High;
        assert!(build_request(&d, &ask("x")).unwrap().body.get("reasoning_effort").is_none());
        for r in Reasoning::ALL {
            assert_eq!(Reasoning::from_id(r.id()), Some(r));
        }
    }

    fn with_files(files: Vec<Attachment>) -> Vec<Message> {
        vec![Message { role: Role::User, text: "mira esto".into(), attachments: files }]
    }

    #[test]
    fn attachments_are_told_apart_by_their_bytes() {
        let png = Attachment::from_file("foto.dat", b"\x89PNG\r\n\x1a\nrest".to_vec()).unwrap();
        assert_eq!((png.kind(), png.mime.as_str()), (AttachKind::Image, "image/png"));
        assert_eq!(Attachment::from_file("a.pdf", b"%PDF-1.7 x".to_vec()).unwrap().kind(), AttachKind::Pdf);
        let csv = Attachment::from_file("puntos.csv", "1,10.5,20.5,100\n".as_bytes().to_vec()).unwrap();
        assert_eq!(csv.kind(), AttachKind::Text);
        // Windows-1252 text from an older program still reads.
        assert_eq!(Attachment::from_file("t.txt", vec![b'a', 0xF1, b'o']).unwrap().kind(), AttachKind::Text);
        assert!(Attachment::from_file("hoja.xlsx", b"PK\x03\x04\0\0".to_vec()).is_err());
        assert!(Attachment::from_file("grande.txt", vec![b'a'; MAX_TEXT_BYTES + 1]).is_err());
        assert_eq!(Attachment::from_file("a\nb.txt", b"x".to_vec()).unwrap().name, "ab.txt");
    }

    #[test]
    fn attachments_go_out_in_each_services_shape() {
        let png = Attachment::from_file("f.png", b"\x89PNGxyz".to_vec()).unwrap();
        let txt = Attachment::from_file("p.csv", b"1,2,3".to_vec()).unwrap();
        let h = with_files(vec![png, txt]);
        let mut s = settings(Provider::Anthropic);
        let c = &build_request(&s, &h).unwrap().body["messages"][0]["content"];
        assert_eq!(c[0]["type"], "image");
        assert_eq!(c[0]["source"]["media_type"], "image/png");
        assert_eq!(c[0]["source"]["data"], "iVBOR3h5eg==");
        assert!(c[1]["text"].as_str().unwrap().contains("[Archivo adjunto: p.csv]"));
        s.provider = Provider::Gemini;
        let parts = &build_request(&s, &h).unwrap().body["contents"][0]["parts"];
        assert_eq!(parts[1]["inline_data"]["mime_type"], "image/png");
        s.provider = Provider::Local;
        let c = &build_request(&s, &h).unwrap().body["messages"][1]["content"];
        assert!(c[1]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
        // DeepSeek reads text only; a PDF is for Anthropic and Gemini.
        s.provider = Provider::DeepSeek;
        assert!(build_request(&s, &h).is_err());
        let pdf = with_files(vec![Attachment::from_file("a.pdf", b"%PDF-1".to_vec()).unwrap()]);
        s.provider = Provider::Local;
        assert!(build_request(&s, &pdf).is_err());
        s.provider = Provider::Anthropic;
        assert_eq!(build_request(&s, &pdf).unwrap().body["messages"][0]["content"][0]["type"], "document");
        // Text goes everywhere, inside the message.
        let only_text = with_files(vec![Attachment::from_file("p.csv", b"1,2".to_vec()).unwrap()]);
        s.provider = Provider::DeepSeek;
        assert!(build_request(&s, &only_text).unwrap().body["messages"][1]["content"].as_str().unwrap().contains("1,2"));
        // Older messages keep only a note about a binary file, never its bytes.
        let mut long = with_files(vec![Attachment::from_file("f.png", b"\x89PNGxyz".to_vec()).unwrap()]);
        long.push(Message::new(Role::Assistant, "ok"));
        long.push(Message::new(Role::User, "y"));
        s.provider = Provider::Anthropic;
        let b = build_request(&s, &long).unwrap().body;
        assert!(b["messages"][0]["content"].as_str().unwrap().contains("[Archivo adjunto en un mensaje anterior: f.png]"));
        let many = with_files((0..=MAX_ATTACHMENTS).map(|i| Attachment::from_file(&format!("{i}.txt"), b"x".to_vec()).unwrap()).collect());
        assert!(build_request(&s, &many).is_err());
    }

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn every_provider_has_a_unique_id_a_key_variable_and_models() {
        for p in Provider::ALL {
            assert_eq!(Provider::from_id(p.id()), Some(p));
            assert!(p.key_variable().ends_with("_API_KEY"));
            assert!(p.suggested_models().is_empty() || p.suggested_models().contains(&p.default_model()));
            assert!(p.key_page().is_empty() || p.key_page().starts_with("https://"));
        }
    }

    #[test]
    fn a_missing_key_or_odd_model_name_is_refused_before_any_network_call() {
        let mut s = settings(Provider::Anthropic);
        s.api_key = "  ".into();
        assert!(build_request(&s, &ask("x")).is_err());
        let mut s = settings(Provider::Gemini);
        s.model = "x/../../evil".into();
        assert!(build_request(&s, &ask("x")).is_err());
        assert!(build_request(&settings(Provider::Anthropic), &[]).is_err());
    }

    #[test]
    fn replies_and_service_errors_are_read() {
        let a = r#"{"content":[{"type":"text","text":"uno "},{"type":"text","text":"dos"}]}"#;
        assert_eq!(parse_reply(Provider::Anthropic, a).unwrap(), "uno dos");
        let g = r#"{"candidates":[{"content":{"parts":[{"text":"hola"}]}}]}"#;
        assert_eq!(parse_reply(Provider::Gemini, g).unwrap(), "hola");
        let e = r#"{"error":{"message":"invalid x-api-key"}}"#;
        assert_eq!(parse_reply(Provider::Anthropic, e).unwrap_err().0, "invalid x-api-key");
        assert!(parse_reply(Provider::Gemini, "{}").is_err());
        assert!(parse_reply(Provider::Gemini, "no json").is_err());
    }

    #[test]
    fn only_cadcraft_blocks_become_proposals_and_bad_ones_are_skipped() {
        let reply = "Prueba:\n```cadcraft\n[{\"command\":\"civil.contours\",\"params\":{\"interval\":1}},{\"nope\":1}]\n```\n```rust\nfn main() {}\n```\n```cadcraft\nnot json\n```\n```cadcraft\n{\"command\":\"bad cmd!\"}\n```";
        let p = proposals(reply);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].command, "civil.contours");
        assert_eq!(p[0].params["interval"], 1);
        assert_eq!(code_blocks(reply).iter().filter(|b| b.lang == "rust").count(), 1);
    }

    #[test]
    fn only_the_latest_messages_are_sent_and_the_key_is_not_in_debug_output() {
        let long: Vec<Message> =
            (0..60).map(|i| Message::new(if i % 2 == 0 { Role::User } else { Role::Assistant }, i.to_string())).chain(ask("fin")).collect();
        let r = build_request(&settings(Provider::Anthropic), &long).unwrap();
        assert_eq!(r.body["messages"].as_array().unwrap().len(), MAX_HISTORY);
        assert!(!format!("{:?}", settings(Provider::Gemini)).contains("sk-test-123"));
    }
}
