<h1>MegaBasterd CLI</h1>

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)

A command-line interface (CLI) fork of [**MegaBasterd**](https://github.com/tonikelope/megabasterd) — the unofficial cross-platform MEGA downloader/uploader/streaming suite by [tonikelope](https://github.com/tonikelope).

> Looking for the GUI version? ➜ [**tonikelope/megabasterd**](https://github.com/tonikelope/megabasterd)

---

## What's different in this CLI version

- **Headless operation** — no GUI required; runs entirely from the command line.
- **Single-file and folder downloads** — supports both individual MEGA file links and folder links.
- **Parallel downloads** — configurable concurrent file downloads for folder links (`--parallel`).
- **Multi-slot chunk downloading** — multiple connections per file for faster throughput (`--slots`).
- **Proxy support** — HTTP proxy with optional authentication.
- **Authenticated downloads** — log in with a MEGA account to access private links.
- **Progress tracking** — real-time percentage and speed displayed in the terminal.

---

## Requirements

- [Java 11 or later](https://adoptium.net/es/temurin/releases/?version=11)

---

## Usage

```
java -jar MegaBasterd.jar --cli --url <mega-link> [options]
```

### Options

| Flag | Short | Description | Default |
|------|-------|-------------|---------|
| `--url` | `-u` | MEGA link (file or folder) | *(required)* |
| `--output` | `-o` | Output directory | `.` (current dir) |
| `--email` | `-e` | MEGA account email | |
| `--password` | `-p` | MEGA account password | |
| `--parallel` | `-n` | Max concurrent file downloads (folder links) | `3` |
| `--slots` | `-s` | Parallel chunk connections per file | `1` |
| `--proxy-host` | | HTTP proxy host | |
| `--proxy-port` | | HTTP proxy port | `8080` |
| `--proxy-user` | | Proxy username | |
| `--proxy-pass` | | Proxy password | |
| `--help` | `-h` | Show help message | |

### Examples

Download a single file:

```bash
java -jar MegaBasterd.jar --cli --url "https://mega.nz/file/XXXXX#YYYYY"
```

Download a folder with 5 parallel downloads and 4 slots per file:

```bash
java -jar MegaBasterd.jar --cli \
  --url "https://mega.nz/folder/XXXXX#YYYYY" \
  --output ./downloads \
  --parallel 5 \
  --slots 4
```

Download with a MEGA account:

```bash
java -jar MegaBasterd.jar --cli \
  --url "https://mega.nz/file/XXXXX#YYYYY" \
  --email user@example.com \
  --password mypassword
```

---

## Building from source

```bash
mvn package -DskipTests
```

The fat JAR is generated at `target/MegaBasterd-<version>-jar-with-dependencies.jar`.

---

## Credits

Based on [MegaBasterd](https://github.com/tonikelope/megabasterd) by [tonikelope](https://github.com/tonikelope).

<p align="center"><b>IMPORTANT:</b> You are not authorized to use MegaBasterd in any way that violates <a href="https://mega.io/es/terms"><b>MEGA's terms of use</b></a>.</p>
