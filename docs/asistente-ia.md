# Asistente IA, procedimientos y complementos de TerraCAD-GIS

Todo está en el menú **Asistente IA** (propio, fuera de Civil-ING).

## 1. Configurar el asistente

**Asistente IA ▸ Asistente IA… ▸ Configuración.**

- **Servicio**: Anthropic (Claude), Google (Gemini), DeepSeek o un motor local (LM Studio, Núcleo IA, Ollama; cualquier API compatible con OpenAI). Cada servicio recuerda su modelo y su clave.
- **Modelo**: escríbelo o elige de la lista; **Actualizar modelos** pregunta al servicio cuáles ofrece hoy (así aparecen los nuevos, por ejemplo los Gemini 4).
- **Razonamiento**: Automático, Bajo, Medio, Alto o Máximo (en Claude, Gemini y los motores locales; DeepSeek decide solo).
- **Adjuntos**: botón *Adjuntar archivo…* (imágenes, PDF y texto: CSV de puntos, LISP, DXF en texto, JSON; máximo 6 por mensaje).
- La clave es tuya: se guarda en el disco solo si lo marcas, y nunca va al registro.

## 2. Procedimientos

Un **procedimiento** es una secuencia de comandos registrados (con parámetros JSON) o de rutinas LISP.

- Cuando el asistente responde con bloques ```` ```cadcraft ```` o ```` ```lisp ````, bajo la respuesta aparecen **Ejecutar todo**, **Probar en una copia** y **Guardar como procedimiento…**.
- **Probar en una copia** ejecuta los pasos, **deshace todo** (dibujo, selección y pasos de deshacer) y deja en el cuadro de chat un informe por paso para que el asistente corrija. Es el ciclo *proponer → correr → probar → mejorar*.
- **Biblioteca de procedimientos**: ejecutar, probar, pedir mejora, poner en la barra, borrar y preparar el parche.
- **Barra del asistente**: botones temporales con los procedimientos marcados; `AI` abre el chat, `●` graba, `≡` abre la biblioteca.

## 3. Grabar una macro

**Iniciar grabación**, haz la tarea, **Detener y analizar grabación**. Se registran tus comandos y sus entradas (puntos, palabras clave, textos, objetos) y lo que ejecutes con parámetros; lo que un comando inicia por dentro no se repite. El asistente recibe la grabación, explica qué intentabas y propone un procedimiento generalizado.

## 4. Pasar un procedimiento a código permanente

**Biblioteca ▸ Preparar parche** genera un `git apply` que añade una línea a `crates/civil/data/procedures.jsonl`; al compilar, el procedimiento queda **incorporado** (aparece en la biblioteca y se puede poner en la barra). El programa nunca aplica el parche: lo revisas y lo aplicas tú.

## 5. Código fuente para el asistente y para otras herramientas

**Asistente IA ▸ Código fuente**: la carpeta se detecta (variable `TERRACAD_SOURCE`, carpeta `source` junto al programa, el repositorio donde se compiló) o la escribes. Con *Enviar el contexto del código fuente con cada pregunta*, el asistente recibe las reglas del proyecto, un mapa de archivos y los archivos más relacionados con tu pregunta (hasta 60 KB). Solo texto de código; nunca claves ni dibujos.

**Crear y copiar el paquete** genera un Markdown (reglas, mapa, qué se pide, procedimiento y parche) para pegarlo en Claude Code, Antigravity, Cursor u otra herramienta y seguir afinando allí.

## 6. Complementos (.tcgplugin)

Un complemento es **un archivo** que añade un menú y una barra de herramientas **sin compilar**.

- **Asistente IA ▸ Gestor de complementos ▸ Instalar complemento…** (o suelta el archivo sobre la ventana). Se muestra una revisión (menú, procedimientos, pasos, código LISP) y solo se instala si pulsas *Instalar*.
- Instalar **no ejecuta nada**. Cada ítem o botón ejecuta su procedimiento al pulsarlo. El código LISP de arranque solo corre si lo permites expresamente, y cada complemento se puede desactivar o quitar.
- **Crear un complemento con mis procedimientos** produce el archivo para compartir.
- Se guardan en la carpeta de configuración, en `plugins/`.

Formato (versión 1): `id`, `name`, `version`, `author`, `description`, `menu`, `items` (`label` + `procedure`), `toolbar` (ids de procedimientos), `procedures` (los de la biblioteca) y `lisp` (`name` + `code`). Ejemplo: `examples/plugins/topografia-rapida.tcgplugin`.
