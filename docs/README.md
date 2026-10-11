# OpenWaveLink: База знаний и Архитектурная документация (Windows 11)

Добро пожаловать в структурированную базу знаний проекта **OpenWaveLink** — полностью открытого, высокопроизводительного матричного аудиомикшера, разработанного **специально и исключительно под Windows 11** как превосходящая замена проприетарного **Elgato Wave Link 3.0**.

---

## 🌟 Ключевые файлы верхнего уровня

- 📖 [**`docs/MASTER_ARCHITECTURE_GUIDE.md`**](./MASTER_ARCHITECTURE_GUIDE.md) — **Генеральное архитектурное руководство**: полный сводный документ проекта (концепция, бескабельный захват, выбор WDM WaveRT драйвера, OpenClipguard 2.0, система управления Windows 11 и технологический стек).
- 🤖 [**`docs/AI_DEVELOPER_PROMPT.md`**](./AI_DEVELOPER_PROMPT.md) — **Специализированный файл для ИИ-разработчиков**: полный системный промпт, техническое задание, сигнатуры Win32/WASAPI API, структуры данных Rust/C++, формулы DSP и правила кодинга для языковых моделей и кодинг-агентов.

---

## 📂 Тематическая структура документации

Все документы сгруппированы по пяти специализированным разделам:

### 1. Исследования Elgato Wave Link и DSP
- [**`docs/01_research/01-wave-link-overview-and-beta.md`**](./01_research/01-wave-link-overview-and-beta.md) — Исследование Elgato Wave Link 3.0: снятие аппаратной привязки к микрофонам Elgato (Hardware Unbundling), матричная таблица роутинга (*Routing Table*), 4 аппаратных входа, 8 софтверных каналов, 5 независимых выходных миксов, технологии *Wave FX* и *Voice Tune*.
- [**`docs/01_research/02-clipguard-dsp-vst-pipeline.md`**](./01_research/02-clipguard-dsp-vst-pipeline.md) — Физика перегрузки АЦП, аппаратный каскад трёх параллельных *Stacked ADCs* в Clipguard 2.0 (135 дБ диапазона), преимущества 32-bit float шины (+770 dBFS запаса), софтверная эмуляция Clipguard для любых микрофонов и хостинг 64-битных VST3-плагинов в Windows 11.

### 2. Аудио-ядро и виртуальные драйверы Windows 11
- [**`docs/02_audio_engine/03-windows11-process-loopback-and-drivers.md`**](./02_audio_engine/03-windows11-process-loopback-and-drivers.md) — Нативный Windows 11 Application Loopback Capture API (`ActivateAudioInterfaceAsync` + `AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS` по PID процесса), архитектура сессий WASAPI, планировщик реального времени Windows MMCSS Pro Audio, устранение дрейфа кварцев USB-устройств через SPSC Ring Buffer.
- [**`docs/02_audio_engine/04-virtual-cables-and-drivers-solutions.md`**](./02_audio_engine/04-virtual-cables-and-drivers-solutions.md) — **Решение проблемы лицензирования виртуальных кабелей**: почему кабели от Elgato и VB-Cable нельзя использовать; почему для входов кабели не нужны вообще (захват по PID); категорический отказ от Варианта В (Steam Audio — бред); детальное сравнение Варианта А (USB/IP) и Варианта Б (WDM WaveRT Sysvad MIT) и **окончательный выбор Варианта Б** (< 1 мс задержки, нативные имена в системе, 100% стабильность).

### 3. Пользовательский интерфейс и управление (Без Stream Deck)
- [**`docs/03_ui_and_control/gui_specification/WAVE_LINK_3_UI_SPEC.md`**](./03_ui_and_control/gui_specification/WAVE_LINK_3_UI_SPEC.md) — 🎨 **Pixel-Perfect спецификация UI Wave Link 3.0**: точная палитра (Deep Purple `#17141f`, карточки `#262130`, акценты `#d97ee0`), динамическая топология матрицы (по умолчанию 1 вход System и 1 выход Personal Mix), всплывающее меню `+ Create channel` с поиском активных приложений Windows 11, создание миксов `+`, модальное окно настройки микса и дублирования выходов, интерактивные кросс-поинты (включение по клику, поканальный Send Gain и Mute) и верхняя панель мониторинга (`Listening to / on`).
- [**`docs/03_ui_and_control/05-windows11-controls-hotkeys-overlay.md`**](./03_ui_and_control/05-windows11-controls-hotkeys-overlay.md) — Нативная система управления под Windows 11 без зависимости от Stream Deck: всплывающий микшер в системном трее (*Taskbar Flyout* в стиле Mica Alt), глобальные хоткеи Win32 `RegisterHotKey`, регулировка громкости колесиком мыши `Alt + ScrollWheel`, боковые кнопки для Push-to-Mute, аппаратный полупрозрачный игровой HUD-оверлей поверх полноэкранных игр и интеграция с мультимедиа-клавишами Windows (SMTC).

### 4. Архитектурный стек и план разработки
- [**`docs/04_architecture_and_roadmap/06-open-source-architecture-and-tech-stack.md`**](./04_architecture_and_roadmap/06-open-source-architecture-and-tech-stack.md) — Системный стек проекта: демон реального времени на Rust (`cpal` + `windows-rs` + `nih-plug`), регистрация в MMCSS Pro Audio, Windows Shared Memory (`CreateFileMappingW`) для 60–144 FPS VU-метров, десктопный UI на Tauri 2.x (< 40 МБ ОЗУ) и COM-интерфейс `IAudioPolicyConfig` для перенаправления игр в один клик.
- [**`docs/04_architecture_and_roadmap/07-feature-comparison-and-roadmap.md`**](./04_architecture_and_roadmap/07-feature-comparison-and-roadmap.md) — Сравнительная матрица Wave Link 3.0 vs OpenWaveLink, формулировка преимуществ для Windows 11 и 5-фазный план разработки от прототипа до MSIX дистрибутива в Winget.

### 5. Практические руководства по реализации
- [**`docs/05_implementation_guides/08-practical-implementation-guide.md`**](./05_implementation_guides/08-practical-implementation-guide.md) — **Практическое руководство на базе реального открытого кода**: разбор кодовой базы `bozbez/win-capture-audio` и форка `Ayrton09` (выживание при перезапуске игр, Exclude Mode, Multi-process mixing), исходный код WDM WaveRT драйвера на базе Microsoft Sysvad и `Virtual-Audio-Driver` (MIT) от MikeTheTech, настройка MMCSS Pro Audio и COM-интерфейс `IAudioPolicyConfigFactory` (по опыту EarTrumpet).
