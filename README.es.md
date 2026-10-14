# Caelestia para Windows

Un shell de escritorio para Windows inspirado en [Caelestia](https://github.com/caelestia-dots/shell),
el shell de Hyprland: una barra vertical con espacios de trabajo, un marco de
pantalla y paneles que crecen desde él (panel, lanzador, utilidades, sesión,
notificaciones), todo coloreado a partir de tu fondo con Material You. Escrito en
Rust con [Slint](https://slint.dev). Lee el mismo `shell.json` que Caelestia, así
que los dots se pueden compartir entre Linux y Windows.

[Read in English](README.md)

| Panel | Centro de notificaciones |
|-------|--------------------------|
| ![Panel con widgets de plugins](docs/screenshots/dashboard.png) | ![Centro de notificaciones](docs/screenshots/sidebar.png) |

| Lanzador: calculadora | Lanzador: portapapeles | Cambio de espacio |
|-----------------------|------------------------|-------------------|
| ![Calculadora](docs/screenshots/launcher-calc.png) | ![Historial del portapapeles](docs/screenshots/launcher-clipboard.png) | ![Indicador estirable](docs/screenshots/active-trail.gif) |

## Funciones

- **Barra** en cada monitor: espacios de trabajo con los iconos de sus apps
  (escritorios virtuales de Windows o espacios de komorebi), ventana activa, reloj,
  iconos de estado con ventanas emergentes (red con las redes Wi-Fi para
  conectarte, audio, Bluetooth…) y campana de notificaciones. Como en
  Hyprland, un video o juego en pantalla completa tapa la barra y el marco de su monitor.
- **Panel**: clima, calendario, multimedia con carátula, CPU/GPU/RAM/disco/red y
  tus propios widgets de plugins.
- **Lanzador** (`Alt+Espacio` o la tecla Windows): búsqueda de apps y
  - `>` acciones (fondo, esquemas con vista previa, claro/oscuro, confeti, energía…),
  - `=` calculadora (`=raiz(16)*pi`, `=90deg`, con coma o punto decimal),
  - `:` emojis en español o inglés (`:fuego`, `:fire`),
  - `;` historial del portapapeles (solo en memoria; ignora los gestores de contraseñas).
- **Centro de notificaciones** con las notificaciones de Windows agrupadas por
  app, y No molestar. Se abre con la campana (un clic fuera lo cierra) o al tocar
  la esquina superior derecha del marco (se cierra al sacar el cursor).
- **Utilidades**: mantener activo, capturas, grabaciones, accesos rápidos y modo rendimiento.
- **Temas** dinámicos desde el fondo (9 variantes) o predefinidos (Catppuccin,
  Gruvbox, Nord…), transparencia y desenfoque.
- **Fondos** con vista previa en vivo y descarga desde cualquier repositorio de GitHub.
- **Galería de dots**: importa la configuración de otra persona desde un enlace
  de GitHub, con opción de deshacer.
- **Plugins**: widgets del panel o de la barra definidos con un `plugin.json` y
  un comando o script ([ejemplos](examples/plugins)).
- **Modo rendimiento**: sin desenfoque, animaciones ni apertura por los bordes con
  batería o con juegos y vídeos a pantalla completa.
- **Mosaico** con [komorebi](https://github.com/LGUG2Z/komorebi), con los colores del esquema.

## Instalación

Requiere Windows 10 de 64 bits (versión 2004 o posterior) o Windows 11.

> **El mosaico necesita komorebi.** Caelestia es la barra, el marco y los paneles;
> el gestor de ventanas en mosaico es [komorebi](https://github.com/LGUG2Z/komorebi)
> (desarrollado y probado con la 0.1.41) y se instala aparte:
>
> ```powershell
> winget install LGUG2Z.komorebi
> ```
>
> Después activa **Usar komorebi para el mosaico** en Ajustes › Paneles › Mosaico.
> Caelestia lo arranca y le aplica tus colores (ver [Mosaico con komorebi](#mosaico-con-komorebi)).
> Sin komorebi el shell funciona igual, usando los escritorios virtuales de Windows como espacios.

**Instalador (recomendado)**: descarga `caelestia-win-<versión>-setup.exe` desde
[Releases](https://github.com/Imsteven0/Hyperland-windows/releases) y ejecútalo.
No necesita permisos de administrador: se instala para tu usuario en
`%LOCALAPPDATA%\Programs\Caelestia` y arranca con Windows (desactívalo en
Ajustes › Paneles › Inicio). Un instalador más nuevo actualiza encima y conserva tus dots.

**Portátil**: descomprime `caelestia-win-<versión>-x64.zip` donde quieras y ejecuta `caelestia.exe`.

**Scoop**:

```powershell
scoop install https://github.com/Imsteven0/Hyperland-windows/releases/latest/download/caelestia.json
```

> **"Windows protegió su PC"**: mientras las versiones no estén firmadas,
> SmartScreen puede avisar la primera vez. Elige **Más información → Ejecutar de
> todas formas**. Cada versión incluye `SHA256SUMS.txt` para comprobar la
> descarga (`Get-FileHash <archivo>`).

Ajustes › Acerca de te avisa cuando hay una versión nueva (lo comprueba en GitHub
una vez al día; pon `general.checkUpdates` en `false` para desactivarlo).

**Desde el código**: necesitas [Rust](https://rustup.rs) 1.92+ y las herramientas de compilación de MSVC, y luego
`powershell -ExecutionPolicy Bypass -File scripts\install.ps1`.

## Atajos

| Atajo | Acción |
|-------|--------|
| `Alt+Espacio` (o la tecla Windows, ver Ajustes › Paneles) | Lanzador |
| `Ctrl+Alt+D` | Panel |
| `Ctrl+Alt+U` | Utilidades |
| `Ctrl+Alt+N` | Centro de notificaciones |
| `Ctrl+Alt+P` | Sesión (bloquear, suspender, reiniciar…) |
| `Ctrl+Alt+S` | Ajustes |
| Ratón en el borde superior / esquina inferior derecha | Panel / utilidades |

En el lanzador: `↑ ↓` o `Tab` para moverte, `Enter` para ejecutar o copiar, `Esc`
para cerrar y `Mayús+Supr` para borrar una entrada del portapapeles. Todos los
atajos se cambian en `windows.shortcuts`.

Desde una terminal: `caelestia toggle launcher|dashboard|utilities|session|sidebar|settings`,
`caelestia launcher calc|emoji|clipboard`, `caelestia confetti`, `caelestia workspace 1..8|next|prev`
(con komorebi, animado como `Alt+1..8`), `caelestia reload`, `caelestia quit`.

## Mosaico con komorebi

1. Instálalo: `winget install LGUG2Z.komorebi` (gratis para uso personal) o con
   el botón **Instalar komorebi** de Ajustes › Paneles › Mosaico.
2. Activa **Usar komorebi para el mosaico**. La barra mostrará los espacios de
   komorebi de cada monitor.

**Animaciones como en la configuración de Hyprland de Caelestia.** Dentro de un
espacio, las ventanas se deslizan a su sitio con las animaciones de movimiento
de komorebi (`windowsMove` de Caelestia: 600 ms con la curva *standard* de
Material). komorebi cambia de espacio al instante, así que el shell reproduce el
cambio: el deslizamiento `workspaces` de Hyprland (500 ms, misma curva), con
miniaturas en vivo de las ventanas sobre tu escritorio real, bajo la barra y el
marco. Elige **Deslizar**, **Vertical**, **Desvanecer** o **Desactivado** en
Ajustes › Paneles › Mosaico (`windows.komorebi.workspaceAnimation`). Ambas siguen
la velocidad de animación y se pausan en el modo rendimiento.

Atajos principales: `Alt+H/J/K/L` enfocar, `Alt+Mayús+H/J/K/L` mover,
`Alt+1..8` ir a un espacio, `Alt+Mayús+1..8` llevar la ventana, `Alt+T` flotante,
`Alt+Q` cerrar, `Alt+P` pausar. La lista completa está en el [README en inglés](README.md#tiling-with-komorebi).

## Configuración

Todo está en `%USERPROFILE%\.config\caelestia\shell.json` (formato de Caelestia),
que se recarga en caliente. Para autocompletado y validación en VS Code añade:

```json
{ "$schema": "https://raw.githubusercontent.com/Imsteven0/Hyperland-windows/main/schema/shell.schema.json" }
```

- **Notificaciones**: activa el *Acceso a notificaciones* para las apps en
  Configuración de Windows › Privacidad y seguridad › Notificaciones.
- **Plugins**: ver [examples/plugins](examples/plugins/README.md).
- **Registro**: `%LOCALAPPDATA%\caelestia\shell.log`.

## Licencia

[GPL-3.0](LICENSE), igual que Caelestia, cuyo diseño y formato de configuración
sigue este proyecto. Las fuentes y bibliotecas incluidas conservan sus licencias:
ver [THIRD_PARTY.md](THIRD_PARTY.md).

Creado por [Steven Araya Gonzalez](https://github.com/Imsteven0). Basado en
[Caelestia](https://github.com/caelestia-dots/shell) del proyecto caelestia-dots.
