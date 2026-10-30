<p align="center">
  <img src="assets/icon-256.png" width="128" height="128" alt="Snowdrift icon">
</p>

<h1 align="center">Snowdrift</h1>

<p align="center">It's snowing on your desktop.</p>

<p align="center">
  <a href="https://github.com/kirDima-787/Snowdrift/releases/latest/download/Snowdrift-Setup.exe"><b>Download for Windows</b></a>
  &nbsp;·&nbsp;
  <a href="https://github.com/kirDima-787/Snowdrift/releases/latest/download/Snowdrift.dmg"><b>Download for macOS</b></a>
  &nbsp;·&nbsp;
  <a href="https://github.com/kirDima-787/Snowdrift/releases/latest/download/Snowdrift-linux-x86_64.tar.gz"><b>Download for Linux</b></a>
  <br>
  <sub>Windows 10 or 11 · macOS 14 or later · Linux · free and open source</sub>
</p>

https://github.com/user-attachments/assets/291c3cd5-3563-4b86-8b66-dfd924f980fc

A lightweight desktop app that brings gentle snowfall to your screen. Snow drifts across your displays, settles on open windows, slips off when they move, and sweeps aside with your cursor.

## Features

- **Natural physics:** Flakes sway in a light breeze, settle into realistic drifts, and slide off windows as you drag or shake them.
- **Interactive:** Push the cursor through drifts to sweep snow aside.
- **Liquid glass:** Native acrylic menu with real backdrop blur, smooth slide animations, and quick toggles.
- **Low footprint:** Hardware-accelerated rendering (Direct3D 11 / Metal), automatically pausing when screens sleep or lock.
- **Bilingual:** Full English and Russian support with one-click switching.

## Controls

All settings live in the system tray / menu bar:

- **Snowfall:** Off, Light, Steady, Heavy, Variable
- **Accumulate:** Build drifts on taskbar and windows
- **Melt Snow:** Gradually melt settled drifts
- **Snow on Windows:** Allow snow on window headers
- **Clear Snow:** Instantly wipe all snow
- **Behind Windows:** Keep snow behind active windows
- **Show in Screenshots:** Include or hide snow in screen captures
- **Open at Login:** Start automatically with the system
- **Language:** English / Russian
- **Check for Updates:** In-app updates via toast notifications

## Building

```sh
# Windows
cargo build --release --bin Snowdrift

# macOS
./build-macos.sh

# Linux
cargo build --release --bin Snowdrift
```

### Windows SmartScreen / Defender

New unsigned Windows builds trigger SmartScreen ("Windows protected your PC") and can be flagged by Defender's heuristics. This is a reputation issue, not a virus. Releases are signed in CI when `WINDOWS_CERT_PFX_BASE64` and `WINDOWS_CERT_PASSWORD` are configured. See [docs/windows-smartscreen.md](docs/windows-smartscreen.md) for the signing setup, Microsoft false-positive submission, and the local development exclusion script.

> Never ship a build signed with `New-SelfSignedCertificate`: Windows does not trust it. Sign `Snowdrift.exe` before building the installer, because the installer carries a copy of the app inside it.

---

## О проекте (RU)

Snowdrift - легкий и атмосферный падающий снег на рабочем столе для Windows, macOS и Linux.

Снежинки плавно кружатся на экране, ложатся в сугробы на панели задач и открытых окнах, тают и разлетаются от движения курсора. Меню и карточки уведомлений выполнены в стиле Liquid Glass с размытием фона и плавной анимацией.

### Быстрый старт
1. Скачайте нужный файл в блоке ссылок выше.
2. Запустите приложение - иконка снежинки появится в системном трее Windows (или строке меню macOS).
3. Кликните по иконке для настройки интенсивности снега, физики сугробов и языка интерфейса.

### Если Windows ругается на Snowdrift

SmartScreen и Defender могут предупреждать о новой неподписанной программе — это вопрос репутации, а не вируса. Подробная инструкция: [docs/windows-smartscreen.md](docs/windows-smartscreen.md). Для локальной разработки есть скрипт `scripts/allow-defender.ps1`, который добавляет узкие исключения только для папок сборки.

---

## Acknowledgements

Inspired by [Snowhush](https://github.com/mbezruchko/Snowhush) and adapted with Liquid Glass aesthetics, native Windows/Linux enhancements, and Russian localization.

## License

[MIT](LICENSE)
