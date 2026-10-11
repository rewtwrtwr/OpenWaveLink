# Выбор решения виртуальных кабелей и аудиодрайверов для Windows 11

## 1. Постановка проблемы: Почему нельзя использовать драйверы Elgato

Оригинальный комплекс **Elgato Wave Link** использует закрытый проприетарный драйвер ядра:
- **`ElgatoWaveLinkAudio.sys`**: виртуальный аудиодрайвер WDM (WaveRT / PortCls).
- Драйвер регистрирует в системе виртуальные конечные точки воспроизведения (*Wave Link System, Game, Voice Chat, Music, Browser, SFX, Aux 1, Aux 2*) и точки записи (*Wave Link Stream, MicrophoneFX*).

### Юридические ограничения:
1. **Драйверы Elgato закрыты**: распространение или реверс-инжиниринг файлов `.sys`, `.inf`, `.cat` от Elgato в стороннем продукте является нарушением EULA и авторских прав.
2. **Коммерческие аналоги (VB-Audio Virtual Cable / Hi-Fi Cable) неприменимы**:
   VB-Audio — это коммерческое закрытое ПО. Их лицензия **прямо запрещает** бесплатное распространение и интеграцию драйверов в open-source проекты без покупки коммерческой лицензии.
3. **Строгая политика Windows 11 (Driver Signature Enforcement)**:
   64-битная Windows 11 категорически блокирует неподписанные драйверы ядра (Code 52). Решение должно иметь прозрачный механизм легальной цифровой подписи (Microsoft WHQL / Attestation Signing).

---

## 2. Главный системный прорыв: Почему для входов кабели не нужны вообще

В старых версиях Windows (7/8/ранняя 10) виртуальные кабели были вынужденной мерой: не было штатного API разделить звук запущенных программ. 

В **Windows 11** эта проблема полностью решена самой Microsoft на уровне ядра:
Официальный API **Windows CoreAudio Application Loopback Capture**:
```cpp
// Windows 11 CoreAudio: Захват звука конкретного процесса (игры, Discord, браузера) напрямую по PID
AUDIOCLIENT_ACTIVATION_PARAMS params = {};
params.ActivationType = AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK;
params.ProcessLoopbackParams.TargetProcessId = targetPID; // PID процесса
params.ProcessLoopbackParams.ProcessLoopbackMode = PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE;

ActivateAudioInterfaceAsync(
    VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    __uuidof(IAudioClient),
    &propVariant,
    completionHandler,
    &asyncOp
);
```

### Вердикт:
Для **80% функционала микшера** (захват звука игр, Discord, плееров, браузеров) **ВИРТУАЛЬНЫЕ КАБЕЛИ НЕ ТРЕБУЮТСЯ ВООБЩЕ!**
OpenWaveLink работает в User-Mode, перехватывая аудиопотоки по PID напрямую из подсистемы Windows 11 с ультранизкой задержкой и нулевой установкой драйверов.

---

## 3. Решение для выходов (Виртуальный микрофон в Discord, Zoom и игры)

Остается задача обеспечения **виртуального микрофона** (*Recording Endpoint*), чтобы внешние приложения (Discord, Zoom, Telegram, соревновательные игры) могли выбрать вход:
- `OpenWaveLink Microphone (Processed Voice / MicFX)`
- `OpenWaveLink Stream Mix`

Рассматривались три потенциальных варианта:

### Вариант В: Использование драйверов Steam Streaming Audio — ИСКЛЮЧЁН (БРЕД)
- **Суть идеи**: Взять драйверы `Steam Streaming Speakers` / `Steam Streaming Microphone`, установленные клиентом Steam.
- **Причина исключения**: **Категорически неприемлемо**.
  1. Привязка к сторонней платформе (пользователь без Steam не сможет пользоваться программой).
  2. Невозможность переименования устройств: в Windows и Discord они отображаются как «Steam Streaming Microphone», что сбивает с толку.
  3. Нестабильность: обновление или удаление клиента Steam может внезапно удалить или повредить драйвер прямо во время работы.
  4. Лицензионные риски использования компонентов Valve вне экосистемы Steam.
- **Статус**: ❌ **ПОЛНОСТЬЮ ОТКЛОНЁН**.

---

### Анализ и сравнение Варианта А и Варианта Б:

| Критерий | Вариант А: Эмуляция USB Audio (USB/IP + `usbaudio.sys`) | Вариант Б: WDM WaveRT драйвер ядра (Microsoft Sysvad / MikeTheTech MIT) |
|---|---|---|
| **Архитектурный принцип** | Эмулирует виртуальную USB-шину через USB/IP; Windows подключает встроенный драйвер `usbaudio.sys`. | Нативный драйвер ядра WDM WaveRT (Real-Time), выделяющий непрерывный циклический буфер памяти в ядре. |
| **Задержка звука (Latency)** | **5–12 мс** (оверхед на упаковку в USB-дескрипторы URB и изохронную буферизацию USB). | **< 1 мс** (прямой DMA-буфер `KSPROPERTY_RTAUDIO_BUFFER` между ядром и WASAPI). |
| **Кастомизация имен и каналов** | Жесткие ограничения USB-дескриптора. Сложно на лету задавать произвольные брендированные имена. | **Полная гибкость**: в INF и топологии прописываются любые имена: `OpenWaveLink Stream Mix`, `OpenWaveLink MicFX` и т.д. |
| **Стабильность при сбоях GUI** | Зависит от юзермодного демона USB/IP. При падении демона Windows играет звук отключения USB-устройства и сбрасывает аудио в Discord. | **Абсолютная надежность**: драйвер загружен в подсистеме Windows Audio (`audiodg.exe`). Перезапуск GUI не прерывает звук в Discord. |
| **Лицензионная чистота** | Открытый стек USB/IP. | **100% Open-Source**: Microsoft Sysvad (MS-PL) + MikeTheTech Virtual-Audio-Driver (**MIT License**). |
| **Цифровая подпись в Windows 11** | Драйвер `usbaudio.sys` подписан Microsoft; для USB/IP шины требуется отдельная подпись. | Процедура **Microsoft Attestation Signing** (бесплатная автоматическая подпись Microsoft для зарегистрированных разработчиков) либо готовый подписанный инсталлятор VirtualDrivers. |

---

## 4. Окончательное архитектурное решение: Выбор ВАРИАНТА Б

На основе технического анализа для проекта OpenWaveLink под Windows 11 **однозначно выбран ВАРИАНТ Б — WDM WaveRT драйвер ядра на базе Microsoft Sysvad / MikeTheTech Virtual-Audio-Driver (MIT)**.

### Почему Вариант Б является лучшим:
1. **WaveRT — это стандарт профессионального звука в Windows**: именно так работают все звуковые карты RME, Focusrite, а также сам оригинальный Elgato Wave Link.
2. **Минимальная задержка**: прямое отображение памяти без сетевых сокетов и без эмуляции USB-пакетов. Задержка составляет менее 1 миллисекунды.
3. **Нативные брендированные имена**: виртуальные устройства отображаются в Windows 11 Sound Settings и Discord как:
   - `OpenWaveLink Stream Mix (Запись / Микрофон)`
   - `OpenWaveLink Microphone FX (Обработанный голос)`
   - `OpenWaveLink Aux Mix`
4. **Бесплатная и открытая лицензия MIT/MS-PL**: код полностью открыт, легален для включения, модификации и свободной сборки.

---

## 5. Вывод звука в OBS Studio: Прямой плагин через Shared Memory (Zero-Driver)

Для вывода звука мастер-микса в **OBS Studio** виртуальный кабель даже не требуется:
- OpenWaveLink создает блок разделяемой памяти Windows:
  `CreateFileMappingW(INVALID_HANDLE_VALUE, NULL, PAGE_READWRITE, 0, BUFFER_SIZE, L"Local\\OpenWaveLink_StreamMix")`.
- Открытый плагин для OBS (`obs-openwavelink-source.dll`) считывает PCM-сэмплы напрямую из памяти со скоростью **< 0.1 мс**.
- Это исключает любые задержки передискретизации Windows и гарантирует максимальное качество трансляции.

---

## 6. Итоговая матрица аудиомаршрутизации OpenWaveLink

| Направление | Источник / Назначение | Используемая технология в Windows 11 | Статус драйвера |
|---|---|---|---|
| **Вход (Capture)** | Игры, Discord, браузер, плееры | **Windows 11 WASAPI Process Loopback (по PID процесса)** | **Ноль драйверов (User-Mode)** |
| **Выход (Stream)** | OBS Studio / Трансляция | **Direct Shared Memory Plugin (`obs-openwavelink-source`)** | **Ноль драйверов (< 0.1 мс)** |
| **Выход (Voice)** | Discord, Zoom, Telegram, Игры | **WDM WaveRT Virtual Mic Driver (Вариант Б, Sysvad MIT)** | **Открытый WDM WaveRT драйвер** |
| **Вход (Mic)** | Физический USB/XLR микрофон | **WASAPI Exclusive / Shared Mode с авто-компенсацией** | Штатный драйвер аудиоинтерфейса |
