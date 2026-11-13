# PlaylistYT

App de escritorio (Rust + egui) en la que un agente de IA crea en tu cuenta de YouTube una playlist oculta con las N canciones más populares de un cantante. Prioriza la versión original de estudio y el audio (YouTube Music o "Official Audio") frente al videoclip.

Escribes el cantante y el número de canciones, pulsas **Crear playlist** y la app te muestra el link de la playlist y la lista de canciones.

> Probado en Windows 10/11. Las API keys se guardan en el Administrador de credenciales de Windows.

## Cómo funciona
1. El LLM elegido (DeepSeek por defecto) lista las N canciones más populares, más 3 de reserva.
2. yt-dlp busca cada canción en YouTube Music (sección canciones) y en YouTube. Esto no gasta cuota.
3. Una heurística descarta directos, covers, remixes, etc., y puntúa a los candidatos.
4. El LLM elige el mejor candidato de cada canción. Solo se aceptan ids que estén en la lista de candidatos.
5. La YouTube Data API crea la playlist (`unlisted`) y añade los vídeos. Coste: 50 + 50·N unidades.

---

## Guía de uso (para quien instala la app)

Necesitas tres cosas: **una API key de un LLM**, **yt-dlp** y **un cliente OAuth de Google** propio. Todo es gratis salvo el LLM, que cuesta céntimos por playlist (o nada si usas Ollama en local).

### 1. Instalar las herramientas
- **Rust** (para compilar): <https://rustup.rs>
- **yt-dlp**:
  ```
  winget install yt-dlp
  ```
- **ffmpeg** (necesario para MP3, el formato por defecto): convierte el audio y añade carátula y etiquetas (artista, álbum, año). Puedes instalarlo desde la propia app (**Ajustes → Instalar ffmpeg**) o con:
  ```
  winget install Gyan.FFmpeg
  ```

**Mantén las dos herramientas al día.** Cuando YouTube cambia algo, las búsquedas o descargas fallan (por ejemplo con "HTTP Error 403"). En **Ajustes** están los botones **Actualizar yt-dlp** (usa pip, winget o `yt-dlp -U`, según cómo lo instalaste) y **Actualizar ffmpeg** (winget).

### 2. Compilar y abrir la app
```
git clone <url-de-este-repo>
cd PlaylistYT
cargo run --release
```
La próxima vez puedes abrir directamente `target\release\PlaylistYT.exe`.

### 3. API key del LLM
1. Crea una API key en tu proveedor. Con DeepSeek: <https://platform.deepseek.com> → **API keys** → **Create new API key** (necesitas algo de saldo).
2. En la app, abre **Ajustes**, elige el proveedor, pega la key en **API key** y pulsa **Guardar**.

Otros proveedores disponibles: OpenAI, Gemini, OpenRouter, Claude, Ollama (local, sin key) o cualquier endpoint compatible con OpenAI ("Personalizado"). La URL base y el modelo se pueden editar.

### 4. Credenciales de Google (una sola vez)
Cada usuario crea su propio cliente OAuth; la app no trae ninguno.

1. Entra en <https://console.cloud.google.com> con la cuenta de YouTube donde quieres las playlists.
2. **Crea un proyecto nuevo** (selector de proyecto, arriba a la izquierda → **Proyecto nuevo** → nombre `PlaylistYT`) y selecciónalo.
3. **Activa la API:** abre `console.cloud.google.com/apis/library/youtube.googleapis.com` y pulsa **Habilitar**.
4. **Pantalla de consentimiento:** menú **APIs y servicios → Pantalla de consentimiento → Comenzar**.
   - Nombre de la app: `PlaylistYT`; correo de asistencia: el tuyo.
   - Público: **Externo**.
   - Acepta la política de datos de usuario y pulsa **Crear**.
5. **Usuario de prueba:** en **Público → Usuarios de prueba → Add users**, añade tu Gmail. Sin esto Google bloquea el acceso. No hace falta publicar la app.
6. **Cliente OAuth:** **Clientes → Crear cliente**, tipo **App de escritorio**, nombre `PlaylistYT` → **Crear** → **Descargar JSON**.
7. Guarda ese archivo en un sitio fijo, por ejemplo `%APPDATA%\PlaylistYT\client_secret.json`. **No lo compartas ni lo subas a ningún repositorio.**

### 5. Conectar con YouTube
1. En **Ajustes**, pon la ruta del `client_secret.json` y pulsa **Guardar**.
2. Pulsa **Conectar con YouTube**. Se abre el navegador:
   - elige la cuenta que añadiste como usuario de prueba;
   - en el aviso "Google no verificó esta app" pulsa **Continuar** (es tu propia app en modo prueba);
   - concede el permiso de YouTube.
3. La barra superior de la app mostrará tu canal como conectado.

En modo de prueba Google caduca el permiso cada 7 días; cuando pase, la app vuelve a abrir el navegador para autorizar.

### 6. Crear una playlist
Escribe el cantante, elige el número de canciones (1–50) y pulsa **Crear playlist**. Al terminar verás el link (con botón de copiar) y la tabla de canciones con su tipo: audio original, audio oficial o vídeo oficial. Las playlists anteriores quedan en el historial de la izquierda.

### 7. Descargar el audio
Descarga solo música sobre la que tengas derechos.

1. En **Ajustes** elige la **carpeta de descargas** (por defecto `Música\PlaylistYT`; cada playlist va a su propia subcarpeta) y el **formato**:
   - **MP3** (por defecto): el formato más compatible (radios de coche, reproductores antiguos, USB). Se convierte a VBR de alta calidad (~245 kbps); convertir no mejora el sonido, solo la compatibilidad. Necesita ffmpeg.
   - **M4A (AAC)**: la pista de audio tal como la sirve YouTube, sin conversión. Suena en móviles, PC, Apple Music y la mayoría de reproductores.
2. Pulsa **Descargar en MP3** (o M4A). Cada archivo se guarda directamente como `Cantante - Canción`, por ejemplo `Tom Odell - Half As Good As You (feat. Alice Merton).mp3`: el agente decide ese nombre al crear la playlist. Con ffmpeg lleva además la carátula del álbum y sus etiquetas. Si se corta, al volver a pulsar solo descarga las que falten (y renombra al mismo formato las que se bajaron con versiones anteriores de la app).
3. **Abrir carpeta** te lleva directamente a los archivos.

### 8. Borrar una playlist
Selecciónala en el historial y pulsa **Borrar playlist** junto al link. Tras confirmar, se borra de tu cuenta de YouTube y del historial (cuesta 50 unidades; no se puede deshacer). Si ya la habías borrado a mano en YouTube, la app solo la quita del historial.

### Cuota de YouTube
Google da 10.000 unidades al día por proyecto. Una playlist de N canciones cuesta 50 + 50·N (10 canciones = 550 u., unas 18 playlists al día). La app lleva la cuenta y muestra cuándo se reinicia (medianoche, hora del Pacífico).

### Problemas frecuentes
| Síntoma | Solución |
|---|---|
| "Acceso bloqueado" al conectar | Tu cuenta no está en **Usuarios de prueba** (paso 4.5). |
| La búsqueda falla o no encuentra nada | Actualiza yt-dlp: botón en Ajustes o `winget upgrade yt-dlp`. |
| Error 401/403 del LLM | API key incorrecta o sin saldo. |
| "Cuota agotada" | Espera al reinicio que indica la barra superior. |

## Privacidad
Nada sensible se guarda dentro de la carpeta del proyecto:
- API keys → Administrador de credenciales de Windows.
- `config.toml`, `quota.json`, `history.json` y `youtube_tokens.json` → `%APPDATA%\PlaylistYT`.
- `client_secret.json` → donde tú lo guardes (el `.gitignore` lo excluye por si acaso).

## Desarrollo
```
cargo test
cargo test -- --ignored --nocapture   # búsqueda real con yt-dlp
cargo clippy
```

## Licencia
[MIT](LICENSE)
