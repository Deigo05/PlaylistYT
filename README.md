# PlaylistYT

App de escritorio para Windows (Rust + egui) en la que un agente de IA crea en tu cuenta de YouTube una playlist oculta con las N canciones más populares de un cantante. Prioriza la versión original de estudio y el audio (YouTube Music o "Official Audio") frente al videoclip. Después puedes descargar el audio de la playlist en MP3 o M4A, con carátula y etiquetas.

Escribes el cantante y el número de canciones, pulsas **Crear playlist** y la app te muestra el link de la playlist y la tabla de canciones.

> Probado en Windows 10/11. Las API keys se guardan en el Administrador de credenciales de Windows, nunca en archivos.

## Funciones
- **Crear playlists** ocultas (`unlisted`) con las canciones más populares de un artista, de 1 a 50.
- **Elegir bien la versión:** descarta directos, covers, remixes, karaoke y versiones aceleradas, y prefiere la pista de audio original.
- **Historial** de las playlists creadas: verlas no gasta cuota.
- **Borrar** una playlist de tu cuenta desde la propia app.
- **Descargar el audio** en MP3 (máxima compatibilidad) o M4A, con nombres `Cantante - Canción`, carátula del álbum y etiquetas (necesita ffmpeg).
- **Actualizar yt-dlp y ffmpeg** desde Ajustes, para cuando YouTube cambia algo.
- **IA intercambiable:** DeepSeek (por defecto), OpenAI, Gemini, OpenRouter, Claude, Ollama (local) o cualquier endpoint compatible con OpenAI.
- **Contador de cuota** de la YouTube Data API, con la hora de reinicio.

## Cómo funciona
1. El LLM elegido lista las N canciones más populares, más 3 de reserva, y decide el nombre limpio de cada archivo (`Cantante - Canción`).
2. yt-dlp busca cada canción en YouTube Music (sección canciones) y en YouTube. Esto no gasta cuota.
3. Una heurística descarta directos, covers, remixes, etc., y puntúa a los candidatos. Solo acepta un vídeo si su título es exactamente el de la canción.
4. El LLM elige el mejor candidato de cada canción. Solo se aceptan ids que estén en la lista de candidatos, así que no puede inventarse vídeos.
5. Si faltan canciones (el LLM a veces inventa títulos de artistas poco conocidos), se completan con las más escuchadas del artista según YouTube Music.
6. La YouTube Data API crea la playlist (`unlisted`) y añade los vídeos. Coste: 50 + 50·N unidades.

---

## Guía de uso: dejar la app funcionando

Necesitas cinco cosas. Todo es gratis salvo el LLM, que cuesta céntimos por playlist (o nada si usas Ollama en local).

| Qué | Para qué | Paso |
|---|---|---|
| Rust | compilar la app | 1 |
| yt-dlp | buscar y descargar | 1 |
| ffmpeg | descargar en MP3 con carátula | 1 |
| API key de un LLM | elegir las canciones | 3 |
| Cliente OAuth de Google | crear playlists en tu cuenta | 4 |

### 1. Instalar las herramientas
Abre PowerShell y ejecuta:
```
winget install Rustlang.Rustup
winget install yt-dlp.yt-dlp
winget install Gyan.FFmpeg
```
Cierra y vuelve a abrir PowerShell para que reconozca los comandos nuevos. Si `cargo` da error al compilar por falta de un "linker", instala también las herramientas de compilación de C++: `winget install Microsoft.VisualStudio.2022.BuildTools` y, en el instalador, marca **Desarrollo para el escritorio con C++**.

ffmpeg también se puede instalar luego desde la propia app (**Ajustes → Instalar ffmpeg**).

**Mantén yt-dlp y ffmpeg al día.** Cuando YouTube cambia algo, las búsquedas o descargas fallan (por ejemplo con "HTTP Error 403"). En **Ajustes** están los botones **Actualizar yt-dlp** (usa pip, winget o `yt-dlp -U`, según cómo lo instalaste) y **Actualizar ffmpeg** (winget).

### 2. Descargar y compilar la app
```
git clone https://github.com/Deigo05/PlaylistYT.git
cd PlaylistYT
cargo run --release
```
La primera compilación tarda unos minutos. Las siguientes veces abre directamente `target\release\PlaylistYT.exe` (puedes crear un acceso directo).

### 3. API key del LLM
1. Crea una API key en tu proveedor. Con DeepSeek: <https://platform.deepseek.com> → **API keys** → **Create new API key** (necesitas algo de saldo; unos pocos dólares dan para cientos de playlists).
2. En la app, abre **Ajustes**, elige el proveedor, pega la key en **API key** y pulsa **Guardar**.

La URL base y el modelo se rellenan solos al elegir el proveedor, y se pueden editar. Con **Ollama (local)** no hace falta key.

### 4. Credenciales de Google (una sola vez)
Cada usuario crea su propio cliente OAuth; la app no trae ninguno. Son unos 10 minutos.

1. Entra en <https://console.cloud.google.com> con la cuenta de YouTube donde quieres las playlists.
2. **Crea un proyecto nuevo** (selector de proyecto, arriba a la izquierda → **Proyecto nuevo** → nombre `PlaylistYT`) y selecciónalo.
3. **Activa la API:** abre <https://console.cloud.google.com/apis/library/youtube.googleapis.com> y pulsa **Habilitar**.
4. **Pantalla de consentimiento:** menú **APIs y servicios → Pantalla de consentimiento → Comenzar**.
   - Nombre de la app: `PlaylistYT`; correo de asistencia: el tuyo.
   - Público: **Externo**.
   - Acepta la política de datos de usuario y pulsa **Crear**.
5. **Usuario de prueba:** en **Público → Usuarios de prueba → Add users**, añade tu Gmail. Sin esto Google bloquea el acceso. No hace falta publicar la app.
6. **Cliente OAuth:** **Clientes → Crear cliente**, tipo **App de escritorio**, nombre `PlaylistYT` → **Crear** → **Descargar JSON**.
7. Guarda ese archivo en un sitio fijo, por ejemplo `%APPDATA%\PlaylistYT\client_secret.json`. **No lo compartas ni lo subas a ningún repositorio.**

### 5. Conectar con YouTube
1. En **Ajustes**, escribe la ruta del `client_secret.json` y pulsa **Guardar**.
2. Pulsa **Conectar con YouTube**. Se abre el navegador:
   - elige la cuenta que añadiste como usuario de prueba;
   - en el aviso "Google no verificó esta app" pulsa **Continuar** (es tu propia app en modo prueba);
   - concede el permiso de YouTube.
3. La barra inferior de la app mostrará tu canal en verde.

En modo de prueba Google caduca el permiso cada 7 días; cuando pase, la app vuelve a abrir el navegador para autorizar.

**Comprobación final:** en la barra inferior deben verse en verde la versión de yt-dlp, la de ffmpeg y tu canal de YouTube. Si alguno sale en naranja, pasa el ratón por encima para ver el motivo.

### 6. Crear una playlist
Escribe el cantante, elige el número de canciones y pulsa **Crear playlist**. En **Progreso del agente** verás cada paso. Al terminar aparecen el link (con botón **Copiar link**) y la tabla de canciones con su tipo: audio original, audio oficial o vídeo oficial. Las playlists anteriores quedan en el historial de la izquierda.

### 7. Descargar el audio
Descarga solo música sobre la que tengas derechos.

1. En **Ajustes** elige la **carpeta de descargas** (por defecto `Música\PlaylistYT`; cada playlist va a su propia subcarpeta) y el **formato**:
   - **MP3** (por defecto): el formato más compatible (radios de coche, reproductores antiguos, USB). Se convierte a VBR de alta calidad (~245 kbps); convertir no mejora el sonido, solo la compatibilidad. Necesita ffmpeg.
   - **M4A (AAC)**: la pista de audio tal como la sirve YouTube, sin conversión. Suena en móviles, PC, Apple Music y la mayoría de reproductores.
2. Pulsa **Descargar en MP3** (o M4A). Cada archivo se guarda como `Cantante - Canción`, por ejemplo `Tom Odell - Half As Good As You (feat. Alice Merton).mp3`, con la carátula del álbum y sus etiquetas.
3. Si se corta, al volver a pulsar solo descarga las que falten. También borra los archivos a medio descargar y renombra al formato correcto lo que se bajó con versiones anteriores.
4. **Abrir carpeta** te lleva directamente a los archivos.

### 8. Borrar una playlist
Selecciónala en el historial y pulsa **Borrar playlist**. Tras confirmar, se borra de tu cuenta de YouTube y del historial (cuesta 50 unidades; no se puede deshacer). Si ya la habías borrado a mano en YouTube, la app solo la quita del historial. Los archivos descargados no se tocan.

### Cuota de YouTube
Google da 10.000 unidades al día por proyecto. Una playlist de N canciones cuesta 50 + 50·N (10 canciones = 550 u., unas 18 playlists al día); borrarla, 50. La barra inferior lleva la cuenta y muestra cuándo se reinicia (medianoche, hora del Pacífico). Es un contador local; el dato oficial está en Google Cloud Console → **APIs y servicios → Cuotas**.

### Problemas frecuentes
| Síntoma | Solución |
|---|---|
| "Acceso bloqueado" al conectar | Tu cuenta no está en **Usuarios de prueba** (paso 4.5). |
| La búsqueda o la descarga falla ("HTTP Error 403") | Actualiza yt-dlp: **Ajustes → Actualizar yt-dlp**. |
| yt-dlp avisa de que falta un "JavaScript runtime" | Instala Deno: `winget install DenoLand.Deno`. |
| No deja elegir MP3 | Falta ffmpeg: **Ajustes → Instalar ffmpeg**, o elige M4A. |
| Error 401/403 del LLM | API key incorrecta o sin saldo. |
| "No encontradas: …" con artistas poco conocidos | El LLM inventó títulos que no existen; la app los sustituye por las canciones más escuchadas del artista. |
| "Cuota agotada" | Espera al reinicio que indica la barra inferior. |

## Privacidad
Nada sensible se guarda dentro de la carpeta del proyecto:
- API keys → Administrador de credenciales de Windows.
- `config.toml`, `quota.json`, `history.json` y `youtube_tokens.json` → `%APPDATA%\PlaylistYT`.
- `client_secret.json` → donde tú lo guardes (el `.gitignore` lo excluye por si acaso).

La app solo habla con el proveedor de IA que elijas, con YouTube (búsquedas con yt-dlp y la YouTube Data API) y con Google para autorizar.

## Desarrollo
```
cargo test                                  # tests unitarios
cargo test -- --ignored --nocapture         # pruebas reales con yt-dlp (sin gastar cuota)
cargo clippy --all-targets
```

| Archivo | Qué hace |
|---|---|
| `src/main.rs` | Arranque de la ventana. |
| `src/app.rs` | Interfaz: formulario, progreso, playlist, historial, ajustes. |
| `src/theme.rs` | Tema visual (colores, botones, secciones numeradas). |
| `src/worker.rs` | Hilo con el runtime de tokio; recibe órdenes de la interfaz y devuelve eventos. |
| `src/agent.rs` | El agente: pide canciones al LLM, busca, elige y crea la playlist. |
| `src/ranking.rs` | Heurística que filtra y puntúa los resultados de búsqueda. |
| `src/llm.rs` | Clientes de LLM (compatible con OpenAI y Anthropic). |
| `src/ytdlp.rs` | Búsqueda y descarga con yt-dlp. |
| `src/youtube.rs` | OAuth y YouTube Data API (crear, añadir, borrar). |
| `src/download.rs` | Descarga de playlists a carpetas, nombres y manifiesto. |
| `src/ffmpeg.rs`, `src/updates.rs` | Localizar ffmpeg y actualizar yt-dlp/ffmpeg. |
| `src/config.rs`, `src/quota.rs`, `src/history.rs` | Ajustes, contador de cuota e historial. |

## Licencia
[MIT](LICENSE)
