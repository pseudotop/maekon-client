<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./assets/brand/logo-full-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="./assets/brand/logo-full-light.svg">
    <img alt="Maekon" src="./assets/brand/logo-full-light.svg" width="400">
  </picture>
</p>

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.ko.md">한국어</a> | <a href="./README.ja.md">日本語</a> | <a href="./README.zh-CN.md">简体中文</a> | <a href="./README.es.md">Español</a>
</p>

# Maekon

> **De la actividad bruta del escritorio a logros diarios de enfoque.**
> Maekon transforma las señales de trabajo locales en una cronología de enfoque en tiempo real y sugerencias accionables.

Un cliente de escritorio para productividad de oficina asistida por IA: captura de contexto local, sugerencias en tiempo real y un panel de control integrado. Desarrollado con Rust y Tauri v2 (shell WebView sobre un frontend React) para rendimiento nativo en macOS, Windows y Linux.

## Instalación en 30 Segundos

macOS / Linux:
```bash
curl -fsSL -o /tmp/oneshim-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh
bash /tmp/oneshim-install.sh
```

Windows (PowerShell):
```powershell
$tmp = Join-Path $env:TEMP "oneshim-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp
```

Para fijar versiones, verificación de firmas y desinstalación:
- Inglés: [`docs/install.md`](./docs/install.md)
- Coreano: [`docs/install.ko.md`](./docs/install.ko.md)

## Por qué Maekon

- **Convierta la actividad en información accionable**: Registre contexto, cronología, tendencias de enfoque e interrupciones en un solo lugar.
- **Manténgase ligero en el dispositivo**: El procesamiento edge (codificación delta, miniaturas, OCR) reduce el volumen de transferencia y mantiene respuestas rápidas.
- **Use una pila de escritorio lista para producción**: Binario multiplataforma, actualización automática, integración con la bandeja del sistema y panel web local.

## Para Quién Es

- Colaboradores individuales que desean visibilidad sobre sus patrones de enfoque y contexto de trabajo
- Equipos que desarrollan herramientas de flujo de trabajo asistidas por IA sobre señales ricas del escritorio
- Desarrolladores que buscan un cliente modular y de alto rendimiento con límites arquitectónicos claros

## Inicio Rápido en 2 Minutos

```bash
# 1) Ejecutar en modo autónomo (recomendado para entornos sensibles a la seguridad)
./scripts/cargo-cache.sh run -p oneshim-app -- --offline

# 2) Abrir el panel local
# http://localhost:10090
```

El modo autónomo está disponible ahora.

El modo conectado está disponible únicamente como una opción de vista previa opt-in.
El modo autónomo sigue siendo la ruta predeterminada lista para producción en versiones de lanzamiento.

## Seguridad y Privacidad de un Vistazo

- Los niveles de filtrado de PII (Desactivado/Básico/Estándar/Estricto) se aplican en la canalización de visión
- Los datos locales se almacenan en SQLite y se gestionan con controles de retención
- Política de informes y respuesta de seguridad: [SECURITY.md](./SECURITY.md)
- Línea base de integridad autónoma: [docs/security/standalone-integrity-baseline.md](./docs/security/standalone-integrity-baseline.md)
- Runbook de operaciones de integridad: [docs/security/integrity-runbook.md](./docs/security/integrity-runbook.md)
- Índice de documentación: [docs/README.md](./docs/README.md)
- Plantillas de guía de automatización: [docs/guides/automation-playbook-templates.md](./docs/guides/automation-playbook-templates.md)
- Runbook de adopción autónoma: [docs/guides/standalone-adoption-runbook.md](./docs/guides/standalone-adoption-runbook.md)
- Guía de los primeros 5 minutos: [docs/guides/first-5-minutes.md](./docs/guides/first-5-minutes.md)
- Contrato de eventos de automatización: [docs/contracts/automation-event-contract.md](./docs/contracts/automation-event-contract.md)
- Contrato de proveedor de IA: [docs/contracts/ai-provider-contract.md](./docs/contracts/ai-provider-contract.md)

## Características

### Características Principales
- **Monitoreo de Contexto en Tiempo Real**: Rastrea ventanas activas, recursos del sistema y actividad del usuario
- **Procesamiento de Imagen Edge**: Captura de pantalla, codificación delta, miniaturas y OCR
- **Funciones de Servidor Conectado (Vista Previa / Opt-in)**: Las sugerencias en tiempo real y la sincronización de retroalimentación están disponibles para validación escalonada y no son la ruta de producción predeterminada
- **Bandeja del Sistema**: Se ejecuta en segundo plano con acceso rápido
- **Actualización Automática**: Actualizaciones automáticas basadas en GitHub Releases
- **Multiplataforma**: Compatible con macOS, Windows y Linux

### Panel Web Local (http://localhost:10090)
- **Panel de Control**: Métricas del sistema en tiempo real, gráficos de CPU/memoria, tiempo de uso de aplicaciones
- **Cronología**: Cronología de capturas de pantalla, filtrado por etiquetas, visor lightbox
- **Informes**: Informes de actividad semanales/mensuales, análisis de productividad
- **Reproducción de Sesión**: Reproducción de sesiones con visualización de segmentos de aplicación
- **Analíticas de Enfoque**: Análisis de enfoque, seguimiento de interrupciones, sugerencias locales
- **Configuración**: Gestión de configuración, exportación/respaldo de datos

### Notificaciones de Escritorio
- **Notificación de Inactividad**: Se activa después de más de 30 minutos de inactividad
- **Notificación de Sesión Prolongada**: Se activa después de más de 60 minutos de trabajo continuo
- **Notificación de Alto Uso**: Se activa cuando el CPU/memoria supera el 90%
- **Sugerencias de Enfoque**: Recordatorios de descanso, programación de tiempo de enfoque, restauración de contexto

## Requisitos

- Rust 1.77.1 o posterior
- macOS 10.15+ / Windows 10+ / Linux (X11/Wayland)

## Inicio Rápido para Desarrolladores (Compilar desde el Código Fuente)

### Compilación

```bash
# Compilar los recursos del panel web embebido (requerido antes de compilaciones de empaquetado/lanzamiento)
./scripts/build-frontend.sh

# Compilación de desarrollo
./scripts/cargo-cache.sh build -p oneshim-app

# Compilación de lanzamiento
./scripts/cargo-cache.sh build --release -p oneshim-app

# Compilar la aplicación de escritorio (Tauri v2, v0.1.5+)
cd src-tauri && cargo tauri build

# Iniciar el servidor de desarrollo con HMR del frontend (v0.1.5+)
cd src-tauri && cargo tauri dev
```

### Caché de Compilación (Recomendado para Desarrollo Local)

```bash
# Opcional: instalar sccache
brew install sccache

# Usar compilaciones Rust con caché mediante el wrapper auxiliar
./scripts/cargo-cache.sh check --workspace
./scripts/cargo-cache.sh test -p oneshim-web
./scripts/cargo-cache.sh build -p oneshim-app
```

Si `sccache` no está instalado, el wrapper recurre a `cargo` normal.

`cargo-cache.sh` también impone límites de tamaño del directorio target para prevenir la saturación del disco local:
- Límite suave (`ONESHIM_TARGET_SOFT_LIMIT_MB`, predeterminado `8192`): limpia `target/debug/incremental`, luego `target/debug/deps` si aún es grande
- Límite duro (`ONESHIM_TARGET_HARD_LIMIT_MB`, predeterminado `12288`): adicionalmente limpia `target/debug/build`
- Poda automática: `ONESHIM_TARGET_AUTO_PRUNE=1` (predeterminado) / `0` (desactivar)
- Estado actual de la caché: `./scripts/cargo-cache.sh --status`

Ejemplo de límites personalizados:
```bash
ONESHIM_TARGET_SOFT_LIMIT_MB=4096 \
ONESHIM_TARGET_HARD_LIMIT_MB=6144 \
./scripts/cargo-cache.sh test --workspace
```

### Ejecución

```bash
# Modo autónomo (recomendado)
./scripts/cargo-cache.sh run -p oneshim-app -- --offline
```

El modo conectado es solo de vista previa y está intencionalmente restringido tras una configuración explícita de servidor/autenticación.
Use el modo autónomo como la ruta de producción predeterminada a menos que su entorno haya validado el modo conectado.

Para sesiones de CI headless o depuración remota donde la inicialización de la bandeja de macOS puede fallar por la ausencia de WindowServer:
```bash
ONESHIM_DISABLE_TRAY=1 ./scripts/cargo-cache.sh run -p oneshim-app -- --offline --gui
```
Use esto solo para rutas de prueba rápida o depuración no interactivas.

### Pruebas

```bash
# Pruebas de Rust
./scripts/cargo-cache.sh test --workspace

# Pruebas E2E — panel web
cd crates/oneshim-web/frontend && pnpm test:e2e

# Lint (política: cero advertencias en CI)
./scripts/cargo-cache.sh clippy --workspace

# Verificación de formato
./scripts/cargo-cache.sh fmt --check

# Verificaciones de calidad de idioma / i18n
./scripts/check-language.sh
# Verificación solo de i18n
./scripts/check-language.sh i18n
# Escaneo de alcance limitado (ejemplo)
./scripts/check-language.sh non-english --path crates/oneshim-web/frontend/src
# Opcional: modo estricto (también falla con advertencias de texto UI hardcodeado)
./scripts/check-language.sh --strict-i18n
```

### Prueba de Humo de WindowServer en macOS (Self-hosted)

Para verificación real de la inicialización de GUI en macOS con una sesión activa de WindowServer, ejecute:
- Workflow: `.github/workflows/macos-windowserver-gui-smoke.yml`
- Etiquetas de runner: `self-hosted`, `macOS`, `windowserver`

## Instalación

Guía de instalación completa:
- Inglés: [`docs/install.md`](./docs/install.md)
- Coreano: [`docs/install.ko.md`](./docs/install.ko.md)

### Instalación Rápida (Terminal)

macOS / Linux:
```bash
curl -fsSL -o /tmp/oneshim-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh
bash /tmp/oneshim-install.sh
```

Windows (PowerShell):
```powershell
$tmp = Join-Path $env:TEMP "oneshim-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp
```

### Recursos de Lanzamiento

Descargue desde [Releases](https://github.com/pseudotop/maekon-client/releases):

Maekon es el nombre visible de la aplicación. Los nombres de archivos de
lanzamiento conservan intencionalmente el formato `oneshim-*` para mantener la
compatibilidad con instaladores, actualizadores y checksums.

| Plataforma | Archivo |
|--------|------|
| macOS Universal (instalador DMG) | `oneshim-macos-universal.dmg` |
| macOS Universal (instalador PKG) | `oneshim-macos-universal.pkg` |
| macOS Universal | `oneshim-macos-universal.tar.gz` |
| macOS Apple Silicon | `oneshim-macos-arm64.tar.gz` |
| macOS Intel | `oneshim-macos-x64.tar.gz` |
| Windows x64 (zip) | `oneshim-windows-x64.zip` |
| Windows x64 (MSI) | `oneshim-app-*.msi` |
| Linux x64 (paquete DEB) | `oneshim-*.deb` |
| Linux x64 | `oneshim-linux-x64.tar.gz` |

## Configuración

### Variables de Entorno

Nota de compatibilidad: las variables `ONESHIM_*`, el comando CLI `oneshim`,
`com.oneshim.client` y las rutas config/data existentes se mantienen como
identificadores técnicos estables en esta línea de lanzamiento.

| Variable | Descripción | Valor Predeterminado |
|------|------|--------|
| `ONESHIM_EMAIL` | Correo electrónico de inicio de sesión (solo modo conectado) | (opcional en autónomo) |
| `ONESHIM_PASSWORD` | Contraseña de inicio de sesión (solo modo conectado) | (opcional en autónomo) |
| `ONESHIM_TESSDATA` | Ruta de datos de Tesseract | (opcional) |
| `ONESHIM_DISABLE_TRAY` | Omitir inicialización de la bandeja del sistema (solo CI headless/prueba de humo remota de GUI) | `0` |
| `RUST_LOG` | Nivel de registro | `info` |

### Archivo de Configuración

`~/.config/oneshim/config.json` (Linux) / `~/Library/Application Support/com.oneshim.agent/config.json` (macOS) / `%APPDATA%\oneshim\agent\config.json` (Windows):

```json
{
  "server": {
    "base_url": "https://api.oneshim.com",
    "request_timeout_ms": 30000,
    "sse_max_retry_secs": 30
  },
  "monitor": {
    "poll_interval_ms": 1000,
    "sync_interval_ms": 10000,
    "heartbeat_interval_ms": 30000
  },
  "storage": {
    "retention_days": 30,
    "max_storage_mb": 500
  },
  "vision": {
    "capture_throttle_ms": 5000,
    "thumbnail_width": 480,
    "thumbnail_height": 270,
    "ocr_enabled": false
  },
  "update": {
    "enabled": true,
    "repo_owner": "pseudotop",
    "repo_name": "maekon-client",
    "check_interval_hours": 24,
    "include_prerelease": false
  },
  "web": {
    "enabled": true,
    "port": 10090,
    "allow_external": false
  },
  "notification": {
    "enabled": true,
    "idle_threshold_mins": 30,
    "long_session_threshold_mins": 60,
    "high_usage_threshold_percent": 90
  }
}
```

## Arquitectura

Un workspace de Cargo con 15 paquetes siguiendo Hexagonal Architecture (Ports & Adapters). Los 14 crates viven bajo `crates/`, y el binario principal/composition root vive en `src-tauri/` (Tauri v2, paquete `oneshim-app`).

```
maekon-client/
├── src-tauri/              # Punto de entrada binario Tauri v2 + composition root
│   ├── src/
│   │   ├── main.rs         # Constructor de la app Tauri + cableado DI
│   │   ├── tray.rs         # Menú de bandeja del sistema
│   │   ├── commands/       # Comandos IPC de Tauri
│   │   └── scheduler/      # Scheduler de fondo
│   └── tauri.conf.json     # Configuración de Tauri
├── crates/
│   ├── oneshim-core/       # Modelos de dominio + traits de port + errores + config
│   ├── oneshim-network/    # HTTP/SSE/WebSocket/gRPC, compresión, auth
│   ├── oneshim-suggestion/ # Recepción y procesamiento de sugerencias
│   ├── oneshim-storage/    # Almacenamiento local SQLite + migraciones de schema
│   ├── oneshim-monitor/    # Métricas del sistema, ventana activa, seguimiento
│   ├── oneshim-vision/     # Captura de pantalla, delta encoding, OCR, filtro PII
│   ├── oneshim-web/        # Panel web local (Axum REST + React)
│   ├── oneshim-automation/ # Automatización, políticas, auditoría
│   ├── oneshim-analysis/   # Pipeline de análisis LLM, clasificación de régimen
│   ├── oneshim-embedding/  # Embeddings vectoriales + cuantización INT8
│   ├── oneshim-audio/      # Captura de audio + pipeline STT
│   ├── oneshim-sandbox-worker/ # Ejecutor sandbox out-of-process
│   ├── oneshim-api-contracts/ # Contratos de tipos API compartidos
│   └── oneshim-lint/       # Herramienta lint del workspace
└── docs/
    ├── crates/             # Documentación detallada por crate
    ├── architecture/       # Documentos ADR (ADR-001~ADR-019; ver docs/architecture/ADR-*.md)
    └── migration/          # Documentos de migración
```

### Documentación de Crates

| Crate | Rol | Documentación |
|----------|------|------|
| oneshim-core | Modelos de dominio, interfaces de port | [Detalles](./docs/crates/oneshim-core.md) |
| oneshim-network | HTTP/SSE/WebSocket/gRPC, compresión, autenticación | [Detalles](./docs/crates/oneshim-network.md) |
| oneshim-vision | Captura, codificación delta, OCR | [Detalles](./docs/crates/oneshim-vision.md) |
| oneshim-monitor | Métricas del sistema, ventanas activas | [Detalles](./docs/crates/oneshim-monitor.md) |
| oneshim-storage | SQLite, almacenamiento offline | [Detalles](./docs/crates/oneshim-storage.md) |
| oneshim-suggestion | Cola de sugerencias, retroalimentación | [Detalles](./docs/crates/oneshim-suggestion.md) |
| oneshim-web | Panel web local, REST API | [Detalles](./docs/crates/oneshim-web.md) |
| oneshim-automation | Control de automatización, registro de auditoría | [Detalles](./docs/crates/oneshim-automation.md) |
| oneshim-analysis | Pipeline de análisis LLM, clasificación de régimen | — |
| oneshim-embedding | Embeddings vectoriales, cuantización INT8 | — |
| oneshim-audio | Captura de audio, pipeline STT | — |
| oneshim-sandbox-worker | Ejecutor de acciones de automatización en sandbox | — |
| oneshim-api-contracts | Contratos de tipos API compartidos | — |
| oneshim-lint | Herramienta lint del workspace (language-check) | — |

Índice completo de documentación: [docs/crates/README.md](./docs/crates/README.md)

Para el flujo de contribución, consulte [CONTRIBUTING.md](./CONTRIBUTING.md).

Las reglas de idioma y consistencia de la documentación se definen en [docs/DOCUMENTATION_POLICY.md](./docs/DOCUMENTATION_POLICY.md).
Traducción al coreano: [README.ko.md](./README.ko.md).
Documento complementario de política en coreano: [docs/DOCUMENTATION_POLICY.ko.md](./docs/DOCUMENTATION_POLICY.ko.md).

## Desarrollo

### Estilo de Código

- **Idioma**: Documentación en inglés como idioma principal, con documentos complementarios en coreano para las guías públicas clave
- **Formato**: Configuración predeterminada de `cargo fmt`
- **Lint**: `cargo clippy` con 0 advertencias

### Agregar Nuevas Características

1. Defina traits de port en `oneshim-core`
2. Implemente adapters en el crate correspondiente
3. Conecte el DI en `src-tauri/src/main.rs`
4. Agregue pruebas

### Compilación de Instaladores

Paquete .app para macOS:
```bash
./scripts/cargo-cache.sh install cargo-bundle
./scripts/cargo-cache.sh bundle --release -p oneshim-app
```

.msi para Windows:
```bash
./scripts/cargo-cache.sh install cargo-wix
./scripts/cargo-cache.sh wix -p oneshim-app
```

## Licencia

Apache License 2.0 — consulte [LICENSE](./LICENSE)

- [Guía de Contribución](./CONTRIBUTING.md)
- [Código de Conducta](./CODE_OF_CONDUCT.md)
- [Política de Seguridad](./SECURITY.md)

## Contribuir

1. Haga un fork
2. Cree una rama de características (`git checkout -b feature/amazing`)
3. Confirme sus cambios (`git commit -m 'Add amazing feature'`)
4. Envíe la rama (`git push origin feature/amazing`)
5. Abra un Pull Request
