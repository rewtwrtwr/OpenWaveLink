# Архитектурная спецификация и технологический стек OpenWaveLink для Windows 11

Данный документ представляет собой полную техническую архитектуру открытого аудиомикшера **OpenWaveLink**, спроектированного исключительно для операционной системы **Windows 11**.

Архитектура оптимизирована под нативные подсистемы Windows 11 Audio Engine, аппаратное ускорение и системные API без лишних кроссплатформенных абстракций. Это обеспечивает минимальную задержку (Ultra-low latency), нулевую нагрузку на процессор и визуальный интерфейс в стиле **Windows 11 Fluent Design (Mica / Acrylic)**.

---

## 1. Системная архитектура OpenWaveLink в среде Windows 11

OpenWaveLink функционирует по сервисной модели:
1. **Фоновый системный звуковой сервис (Audio Engine Service / Daemon)**: высокоприоритетный процесс на Rust/C++, взаимодействующий с WASAPI и аудиодрайвером WDM WaveRT.
2. **Пользовательский интерфейс (Fluent Design UI)**: окно на базе Tauri 2.x (WebView2) или нативного WinUI 3 с поддержкой материалов Mica Alt, закругленных углов Windows 11 и плавной кинетики Framer Motion.
3. **Высокоскоростная разделяемая память (Windows Shared Memory)**: передача телеметрии VU-метров с частотой 60–120 FPS напрямую в GPU/UI без накладных расходов на JSON или сетевые сокеты.
4. **Подсистема управления процессами**: интеграция с Win32 Shell COM API (`IAudioPolicyConfig`) для автоматической поканальной маршрутизации игр и приложений.

```
+===================================================================================+
|                          WINDOWS 11 APPLICATION ECOSYSTEM                         |
|                                                                                   |
|  [CS2 / Cyberpunk 2077]    [Discord Voice Chat]    [Spotify Music]   [Google Chrome]|
|  (Process ID: 10420)       (Process ID: 8124)      (Process ID: 3904)(Process ID:..)|
|          |                         |                       |                 |    |
+==========|=========================|=======================|=================|====+
           |                         |                       |                 |
+==========v=========================v=======================v=================v====+
|             WINDOWS 11 WASAPI PROCESS-LOOPBACK & WDM WAVERT DRIVERS               |
|                                                                                   |
|  +-----------------------------------------------------------------------------+  |
|  | AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK (Per-Process Audio Streams)    |  |
|  | + Virtual WDM WaveRT Audio Cable Endpoints (Game, Chat, Music, Browser)     |  |
|  +---------------------------------------+-------------------------------------+  |
+==========================================|========================================+
                                           | Audio Buffers (48 kHz, 24/32-bit Float)
+==========================================v========================================+
|                       OPENWAVELINK CORE ENGINE (RUST / WIN32)                     |
|                                                                                   |
|  +-----------------------------------------------------------------------------+  |
|  | REAL-TIME AUDIO PROCESSING PIPELINE (Hard Realtime, MMCSS Pro Audio Priority)|  |
|  |                                                                             |  |
|  |  [Channel In]       [DSP Chain]              [Matrix Summing Engine]        |  |
|  |  Ch 1: Mic     --> [OpenClipguard 2.0] ---> +---------------------------+  |  |
|  |  Ch 2: Game    --> [VST3 Plugin Rack ] ---> | Gain Matrix               |  |  |
|  |  Ch 3: Discord --> [Stereo Panner    ] ---> | (N Channels x M Mixes)    |  |  |
|  |  Ch 4: Music   --> [Channel Fader    ] ---> +-------------+-------------+  |  |
|  |                                                           |                |  |
|  |                                                           v                v  |  |
|  |                                                     [Stream Mix]    [Monitor] |  |
|  +---------------------------------------+-------------------+-----------------+  |
|                                          |                   |                    |
|  +---------------------------------------|-------------------|-----------------+  |
|  | CONTROL & TELEMETRY SUBSYSTEM         |                   |                 |  |
|  | - Windows Shared Memory (CreateFileMappingW: VU Meters @ 60-120 FPS)        |  |
|  | - Win32 Hotkeys & Input Hook Subsystem (RegisterHotKey / WH_KEYBOARD_LL)    |  |
|  | - Windows Audio Policy COM Interop (IAudioPolicyConfigFactory)              |  |
|  | - DirectComposition Topmost In-Game HUD Overlay                             |  |
|  +---------------------------------------+-------------------------------------+  |
+==========================================|========================================+
                                           | Zero-Copy Shared Memory (VU)
                                           | & Win32 IPC (Commands)
+==========================================v========================================+
|                       WINDOWS 11 USER INTERFACE (FLUENT DESIGN)                   |
|                                                                                   |
|  +------------------------------------+  +-------------------------------------+  |
|  | Desktop Matrix Mixer Window        |  | Taskbar Flyout Tray Mixer           |  |
|  | - Windows 11 Mica Alt Material     |  | - Acrylic Blur Backdrop             |  |
|  | - Rounded Corners (DWMWCP_ROUND)   |  | - Direct Shell_NotifyIcon Alignment |  |
|  | - React 19 + Framer Motion (60 FPS)|  | - Quick Mute & Output Endpoint Swop |  |
|  +------------------------------------+  +-------------------------------------+  |
+===================================================================================+
```

---

## 2. Сравнение технологических стеков аудио-ядра для Windows 11

В среде Windows 11 наивысший приоритет отдается надежности вывода звука без задержек (Audio Glitch Resilience) и совместимости с мультимедийным планировщиком задач Windows (MMCSS).

### 2.1. Сравнительная матрица аудио-стеков

| Критерий | Rust (`cpal` + `windows-rs` + `nih-plug`) | C++ (`JUCE` Framework) | C# (.NET 10 / NAudio / WASAPI) | Electron / Node.js |
|---|---|---|---|---|
| **Управление памятью** | **Zero-Allocation Realtime Thread**, безопасность компилятора | Ручное управление (риск UB и memory leaks) | Garbage Collector (GC вызывает пропуски сэмплов) | Garbage Collector (V8 Engine) |
| **Интеграция с Windows 11 API** | Идеальная через крейт `windows-rs` (полный Win32/WinRT SDK) | Требует ручных COM-оберток | Встроенная через P/Invoke | Затруднительна (через нативные аддоны C++) |
| **MMCSS Регистрация** | `AvSetMmThreadCharacteristicsW("Pro Audio")` | Поддерживается | Затруднена из-за потоков GC | Не поддерживается в JS-потоке |
| **Хостинг VST3-эффектов** | Отлично через `nih-plug` (с изоляцией падений) | Отлично (модуль `juce_audio_processors`) | Сложно / сторонние библиотеки | Крайне медленно |
| **Лицензия** | **MIT / Apache-2.0** (100% свободно) | GPLv3 или коммерческая ($$$) | MIT | MIT |
| **Потребление ОЗУ** | **18 – 35 МБ** | 30 – 50 МБ | 80 – 150 МБ | 250 – 450 МБ |

### 2.2. Архитектурный выбор: Rust + `windows-rs` + `nih-plug`
- **MMCSS Pro Audio Threading**: звуковой поток регистрируется в подсистеме Windows Multimedia Class Scheduler Service с профилем `"Pro Audio"`, что гарантирует выделение квантов процессора в обход фоновых служб Windows.
- **Безопасная изоляция VST3**: сторонние плагины запускаются в изолированных воркерах, защищая основной звуковой тракт стримера от падений.
- **OpenClipguard 2.0**: полностью программный алгоритм предотвращения перегрузки микрофона с опережением 1.5 мс и SIMD-векторизацией (AVX2), работающий с любым USB или XLR интерфейсом в Windows 11.

---

## 3. Архитектура пользовательского интерфейса: Windows 11 Fluent Design

Пользовательский интерфейс OpenWaveLink спроектирован так, чтобы выглядеть как штатное приложение Windows 11 уровня настроек системы и Microsoft Teams, но с максимальной отзывчивостью профессионального аудио-пульта.

### 3.1. Технологический стек UI
- **Оболочка окна**: **Tauri 2.x** на базе нативного системного движка **Microsoft Edge WebView2**. В Windows 11 компонент WebView2 предустановлен на уровне ОС и обновляется централизованно через Windows Update, благодаря чему инсталлятор OpenWaveLink весит всего **~18 МБ** (в отличие от Electron весом 150+ МБ).
- **Материал Mica Alt и Acrylic**: 
  Использование атрибутов Desktop Window Manager (DWM):
  ```rust
  // Активация нативного фона Mica Alt в окне Windows 11 через Win32 DWM API
  use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE};
  
  const DWMSBT_TABBEDWINDOW: u32 = 4; // Mica Alt
  unsafe {
      DwmSetWindowAttribute(
          hwnd,
          DWMWA_SYSTEMBACKDROP_TYPE,
          &DWMSBT_TABBEDWINDOW as *const _ as _,
          std::mem::size_of::<u32>() as u32,
      );
  }
  ```
- **Скругленные углы и темная тема**: автоматическое применение системного скругления `DWMWCP_ROUND` и отслеживание смены темы ОС через реестр (`AppsUseLightTheme`).
- **Стек рендеринга матрицы**: React 19 + Tailwind CSS + Framer Motion. Матрица каналов отрисовывается на базе CSS Grid, а движение регуляторов громкости аппаратно ускоряется видеокартой (DirectX Compositor).

---

## 4. Высокоскоростное межпроцессное взаимодействие (Windows Shared Memory)

Для мгновенной визуализации индикаторов громкости (VU-метров) без нагрузки на процессор реализован прямой канал разделяемой памяти Windows:

### 4.1. Механизм `CreateFileMappingW`
Звуковое ядро и интерфейс создают именованный блок памяти в пространстве Windows:
- Имя отображения: `Local\OpenWaveLink_Telemetry_V1`
- Доступ: чтение/запись на стороне ядра, только чтение (`PAGE_READONLY`) на стороне UI.
- Структура блока:

```rust
#[repr(C)]
pub struct AudioTelemetryBlock {
    pub magic: u32,             // 0x57415645 ("WAVE")
    pub sequence: u32,          // Инкрементный счетчик кадров
    pub timestamp_qpc: u64,     // Метка времени QueryPerformanceCounter
    pub channel_count: u32,     // Число активных каналов
    pub channels: [ChannelTelemetry; 16],
    pub mix_count: u32,         // Число миксов
    pub mixes: [MixTelemetry; 8],
}

#[repr(C)]
pub struct ChannelTelemetry {
    pub peak_l: f32,            // Пиковый уровень левого канала [0.0..1.0]
    pub peak_r: f32,            // Пиковый уровень правого канала [0.0..1.0]
    pub rms_l: f32,             // Среднеквадратичный уровень RMS L
    pub rms_r: f32,             // Среднеквадратичный уровень RMS R
    pub is_clipping: u32,       // Флаг срабатывания клиппинга
}
```

UI опрашивает этот буфер через `requestAnimationFrame` синхронно с частотой обновления монитора (60 Гц, 144 Гц или 240 Гц), обеспечивая абсолютную плавность индикаторов без сериализации JSON.

---

## 5. Низкоуровневая интеграция с аудио-подсистемами Windows 11

### 5.1. Захват звука приложений (WASAPI Process-Loopback)
Начиная с Windows 10 (20H1) и во всех версиях Windows 11 доступен официальный интерфейс захвата звука конкретных процессов:
- Интерфейс: `IAudioClient` активируется через `ActivateAudioInterfaceAsync`.
- Параметры активации: `AUDIOCLIENT_ACTIVATION_PARAMS` со структурой `PROCESS_LOOPBACK_PARAMS`:
  ```rust
  let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
      ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
      ProcessLoopbackParams: PROCESS_LOOPBACK_PARAMS {
          TargetProcessId: target_pid,
          ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_PROCESS_TREE, // Захватывать дочерние процессы
      },
  };
  ```
Это позволяет OpenWaveLink изолированно забирать чистый звук игры или браузера, даже если в Windows они выводят звук на общее устройство по умолчанию.

### 5.2. Виртуальный аудиодрайвер WDM WaveRT (Virtual-Audio-Driver)
Для создания выделенных виртуальных входных устройств в Windows (например, *OpenWaveLink Game Input*, *OpenWaveLink Aux*) используется легковесный виртуальный WDM-драйвер на базе открытого стандарта **Sysvad / WaveRT**:
- Драйвер регистрирует в диспетчере устройств Windows системные конечные точки WaveRT.
- Поддерживает режимы низкой задержки с кольцевым циклическим буфером (Circular DMA Buffer).
- Имеет цифровую подпись (WHQL / Test-Signed режим для разработки).

### 5.3. Автоматическая маршрутизация через Windows Audio Policy
Чтобы пользователю не приходилось открывать «Параметры Windows $\rightarrow$ Дополнительные параметры звука» для назначения каждого приложения на свой канал, OpenWaveLink использует интерфейс COM:
- `IAudioPolicyConfigFactory` / `IAudioPolicyConfig`
- Метод: `SetPersistedDefaultAudioEndpoint(process_id, eConsole, endpoint_id)`
При назначении игры на канал в интерфейсе OpenWaveLink системная маршрутизация Windows мгновенно обновляется автоматически.

---

## 6. Модульная структура репозитория OpenWaveLink для Windows 11

```
OpenWaveLink/
├── Cargo.toml                      # Корневой Rust workspace
├── crates/
│   ├── owl-core/                   # Звуковое ядро реального времени
│   │   ├── src/
│   │   │   ├── engine.rs           # Realtime аудио-луп и MMCSS Pro Audio
│   │   │   ├── matrix.rs           # N-входов на M-миксов сумматор
│   │   │   ├── clipguard.rs        # OpenClipguard 2.0 (Lookahead Limiter)
│   │   │   └── vst_host.rs         # Хостинг VST3 на базе nih-plug
│   ├── owl-backend-wasapi/         # Драйвер захвата звука Windows 11
│   │   ├── src/
│   │   │   ├── process_loopback.rs # WASAPI Process-Loopback захват по PID
│   │   │   ├── device_manager.rs   # IMMDeviceEnumerator и WASAPI Endpoints
│   │   │   └── policy_config.rs    # COM-интерфейс IAudioPolicyConfig
│   ├── owl-controls/               # Управление под Windows 11
│   │   ├── src/
│   │   │   ├── tray.rs             # Shell_NotifyIconW и всплывающий Flyout
│   │   │   ├── hotkeys.rs          # Win32 RegisterHotKey & WH_KEYBOARD_LL
│   │   │   ├── mouse_hook.rs       # WH_MOUSE_LL для Alt+ScrollWheel
│   │   │   └── overlay.rs          # DirectComposition плавающий HUD поверх игр
│   ├── owl-ipc/                    # Межпроцессное взаимодействие
│   │   ├── src/
│   │   │   └── shared_memory.rs    # Windows CreateFileMappingW (VU-метры)
│   └── owl-daemon/                 # Основная системная служба / процесс трея
├── driver/                         # WDM WaveRT виртуальный аудиодрайвер
│   └── sysvad/                     # Исходный код C++ драйвера Windows DDK
├── frontend/                       # Пользовательский интерфейс Fluent Design
│   ├── src/
│   │   ├── components/             # MatrixGrid, ChannelStrip, TrayFlyout, InGameHUD
│   │   ├── store/                  # Zustand Store для состояния микшера
│   │   └── App.tsx                 # Главное окно микшера (Mica Alt)
└── tauri-app/                      # Windows 11 Desktop оболочка Tauri 2.x
    └── src-tauri/
        └── tauri.conf.json         # Настройки окна, WebView2 и трея
```

---

## 7. Вывод

Ориентация исключительно на **Windows 11** позволяет исключить любые компромиссы кроссплатформенности. OpenWaveLink использует самые мощные нативные технологии Microsoft — **WASAPI Process-Loopback, MMCSS Pro Audio, Shared Memory, Fluent Design Mica Alt и DirectComposition Overlay**, обеспечивая недостижимый для кроссплатформенных аналогов уровень производительности, интеграции и удобства.
