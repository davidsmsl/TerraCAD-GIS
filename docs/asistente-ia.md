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

## Probar la conexión

El botón **Probar conexión** (en la barra del asistente y en Configuración) y el comando **Asistente de IA ▸ Probar conexión** hacen dos pasos: listan los modelos del servicio (comprueban la dirección y la clave) y envían una solicitud mínima al modelo elegido (comprueban que la clave puede usarlo y miden el tiempo). El informe nunca incluye la clave.

## Respuestas largas y memoria

- El límite de respuesta subió a 8192 tokens. Si el servicio corta la respuesta por longitud, la aplicación le pide continuar sola (hasta 3 veces) y une las partes.
- Cada solicitud lleva los últimos 24 mensajes de la conversación, y la conversación se guarda entre sesiones (`civil-ai-chat.json`, solo legible por el usuario, máximo 60 líneas, sin archivos adjuntos). «Nueva conversación» la borra.

## Que el asistente modifique la aplicación

1. Pide el cambio. El asistente responde con un parche `diff` (git, rutas desde la raíz del proyecto).
2. Bajo el bloque aparece **Aplicar al código fuente…**: muestra los archivos que toca y si se aplica limpio. Al confirmar, se aplica en la carpeta del código fuente (Asistente de IA ▸ Código fuente) y se guarda como un commit propio («Asistente: …») que solo incluye esos archivos; se deshace con `git revert`.
3. Aparece un aviso con **Recompilar y reiniciar** (`cargo build --release -p cadcraft`, necesita Rust), **Reiniciar sin recompilar** o dejarlo para después. El aviso se guarda (`assistant-pending.json`) y reaparece en la próxima sesión hasta que se recompile.
4. Al reiniciar, la aplicación pregunta primero por los dibujos sin guardar; si cancelas, no se reinicia. Después de reiniciar la conversación continúa donde quedó.

Los parches con rutas fuera del proyecto, dentro de `.git` o con `..` se rechazan.

## Conectar GitHub

En Configuración ▸ GitHub escribe el repositorio (`propietario/nombre`), la rama (por defecto `terracad-gis-asistente`) y un token personal con permiso **Contents: Read and write** solo sobre ese repositorio. **Probar GitHub** valida el token y el permiso de escritura; **Guardar avance ahora** hace commit de lo que haya cambiado y lo envía a esa rama sin tocar tus ramas ni tus remotos; la casilla «Guardar en GitHub después de cada cambio» lo hace automáticamente.

El token se pasa a git por variables de entorno (no por la línea de comandos), se oculta de todos los mensajes y solo se guarda en disco si marcas «Recordar las claves» (también puede venir de `TERRACAD_GITHUB_TOKEN` o `GITHUB_TOKEN`).
