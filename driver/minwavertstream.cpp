#include "minwavertstream.h"
#include "minwavert.h"

#pragma code_seg("PAGE")

CMiniportWaveRTStream::CMiniportWaveRTStream(CMiniportWaveRT* miniport, ULONG pinId, BOOLEAN bCapture)
    : m_cRef(1),
      m_pMiniport(miniport),
      m_PinId(pinId),
      m_bCapture(bCapture),
      m_State(KSSTATE_STOP),
      m_pFormat(NULL),
      m_pMdl(NULL),
      m_pDmaBuffer(NULL),
      m_DmaBufferSize(0),
      m_PositionRegister(0),
      m_pHwPositionRegister(NULL),
      m_NotificationEventsCount(0),
      m_TotalFramesProcessed(0)
{
    RtlZeroMemory(m_pNotificationEvents, sizeof(m_pNotificationEvents));
    m_QpcFrequency = KeQueryPerformanceCounter(NULL);
    m_StartTime.QuadPart = 0;
}

CMiniportWaveRTStream::~CMiniportWaveRTStream() {
    if (m_pDmaBuffer) {
        FreeAudioBuffer(m_pMdl, m_DmaBufferSize);
    }
    if (m_pFormat) {
        ExFreePoolWithTag(m_pFormat, OWL_POOL_TAG);
        m_pFormat = NULL;
    }
}

// IUnknown implementation
STDMETHODIMP CMiniportWaveRTStream::QueryInterface(REFIID InterfaceId, PVOID* Interface) {
    if (!Interface) return STATUS_INVALID_PARAMETER;

    if (IsEqualGUID(InterfaceId, IID_IUnknown) ||
        IsEqualGUID(InterfaceId, IID_IMiniportWaveRTStream) ||
        IsEqualGUID(InterfaceId, IID_IMiniportWaveRTStreamNotification)) {
        *Interface = static_cast<PVOID>(static_cast<IMiniportWaveRTStreamNotification*>(this));
        AddRef();
        return STATUS_SUCCESS;
    }

    *Interface = NULL;
    return STATUS_NOINTERFACE;
}

STDMETHODIMP_(ULONG) CMiniportWaveRTStream::AddRef() {
    return InterlockedIncrement(&m_cRef);
}

STDMETHODIMP_(ULONG) CMiniportWaveRTStream::Release() {
    LONG count = InterlockedDecrement(&m_cRef);
    if (count == 0) {
        delete this;
    }
    return count;
}

// IMiniportWaveRTStream methods
STDMETHODIMP_(NTSTATUS) CMiniportWaveRTStream::SetFormat(PKSDATAFORMAT DataFormat) {
    PAGED_CODE();
    if (!DataFormat) return STATUS_INVALID_PARAMETER;

    if (m_State != KSSTATE_STOP) {
        return STATUS_INVALID_DEVICE_STATE;
    }

    if (m_pFormat) {
        ExFreePoolWithTag(m_pFormat, OWL_POOL_TAG);
        m_pFormat = NULL;
    }

    ULONG formatSize = DataFormat->FormatSize;
    m_pFormat = (PKSDATAFORMAT_WAVEFORMATEXTENSIBLE)ExAllocatePool2(
        POOL_FLAG_PAGED, formatSize, OWL_POOL_TAG
    );

    if (!m_pFormat) return STATUS_INSUFFICIENT_RESOURCES;
    RtlCopyMemory(m_pFormat, DataFormat, formatSize);

    return STATUS_SUCCESS;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveRTStream::SetState(KSSTATE State) {
    PAGED_CODE();

    switch (State) {
    case KSSTATE_STOP:
        m_State = KSSTATE_STOP;
        m_PositionRegister = 0;
        if (g_pSharedBridge) g_pSharedBridge->SetPinState(m_PinId, KSSTATE_STOP);
        break;

    case KSSTATE_ACQUIRE:
        m_State = KSSTATE_ACQUIRE;
        if (g_pSharedBridge) g_pSharedBridge->SetPinState(m_PinId, KSSTATE_ACQUIRE);
        break;

    case KSSTATE_PAUSE:
        m_State = KSSTATE_PAUSE;
        if (g_pSharedBridge) g_pSharedBridge->SetPinState(m_PinId, KSSTATE_PAUSE);
        break;

    case KSSTATE_RUN:
        m_State = KSSTATE_RUN;
        m_StartTime = KeQueryPerformanceCounter(NULL);
        if (g_pSharedBridge) g_pSharedBridge->SetPinState(m_PinId, KSSTATE_RUN);
        break;

    default:
        return STATUS_INVALID_PARAMETER;
    }

    return STATUS_SUCCESS;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveRTStream::AllocateAudioBuffer(
    ULONG               RequestedSize,
    PMDL*               AudioBufferMdl,
    ULONG*              ActualSize,
    ULONG*              OffsetFromLo,
    MEMORY_CACHING_TYPE* CacheType
) {
    PAGED_CODE();
    if (!AudioBufferMdl || !ActualSize || !OffsetFromLo || !CacheType) {
        return STATUS_INVALID_PARAMETER;
    }

    // Align size to page boundary
    ULONG size = (RequestedSize + PAGE_SIZE - 1) & ~(PAGE_SIZE - 1);
    if (size < OWL_MIN_BUFFER_SIZE) size = OWL_MIN_BUFFER_SIZE;
    if (size > OWL_MAX_BUFFER_SIZE) size = OWL_MAX_BUFFER_SIZE;

    PHYSICAL_ADDRESS lowest = { 0 };
    PHYSICAL_ADDRESS highest = { 0xFFFFFFFF, 0 }; // 4GB Boundary for DMA
    PHYSICAL_ADDRESS boundary = { 0 };

    m_pDmaBuffer = MmAllocateContiguousMemorySpecifyCache(
        size, lowest, highest, boundary, MmCached
    );

    if (!m_pDmaBuffer) {
        return STATUS_INSUFFICIENT_RESOURCES;
    }

    RtlZeroMemory(m_pDmaBuffer, size);

    m_pMdl = IoAllocateMdl(m_pDmaBuffer, size, FALSE, FALSE, NULL);
    if (!m_pMdl) {
        MmFreeContiguousMemory(m_pDmaBuffer);
        m_pDmaBuffer = NULL;
        return STATUS_INSUFFICIENT_RESOURCES;
    }

    MmBuildMdlForNonPagedPool(m_pMdl);

    m_DmaBufferSize = size;
    *AudioBufferMdl = m_pMdl;
    *ActualSize = size;
    *OffsetFromLo = 0;
    *CacheType = MmCached;

    // Register this buffer in the global shared bridge
    if (g_pSharedBridge) {
        g_pSharedBridge->InitializePin(m_PinId, m_pDmaBuffer, size, &m_PositionRegister);
    }

    return STATUS_SUCCESS;
}

STDMETHODIMP_(VOID) CMiniportWaveRTStream::FreeAudioBuffer(PMDL AudioBufferMdl, ULONG BufferSize) {
    PAGED_CODE();
    UNREFERENCED_PARAMETER(BufferSize);

    if (g_pSharedBridge) {
        g_pSharedBridge->CleanupPin(m_PinId);
    }

    if (AudioBufferMdl) {
        IoFreeMdl(AudioBufferMdl);
        m_pMdl = NULL;
    }

    if (m_pDmaBuffer) {
        MmFreeContiguousMemory(m_pDmaBuffer);
        m_pDmaBuffer = NULL;
    }

    m_DmaBufferSize = 0;
}

STDMETHODIMP_(VOID) CMiniportWaveRTStream::GetHardwareLatency(PKSRTAUDIO_HWLATENCY Latency) {
    PAGED_CODE();
    if (!Latency) return;

    // Ultra-low latency reporting (< 1 ms)
    Latency->FifoSize = 256;      // 256 samples
    Latency->ChipWriteDelay = 0;  // Direct RAM mapping
    Latency->ChipReadDelay = 0;
    Latency->CodecDelay = 0;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveRTStream::GetPositionRegister(PKSRTAUDIO_HWREGISTER Register) {
    PAGED_CODE();
    if (!Register) return STATUS_INVALID_PARAMETER;

    // Direct position register mapped to audiodg.exe user-space
    Register->Register = &m_PositionRegister;
    Register->Width = 32;
    Register->Numerator = 1;
    Register->Denominator = 1;
    Register->Accuracy = 0;

    return STATUS_SUCCESS;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveRTStream::GetClockRegister(PKSRTAUDIO_HWREGISTER Register) {
    PAGED_CODE();
    if (!Register) return STATUS_INVALID_PARAMETER;

    Register->Register = (PVOID)&m_TotalFramesProcessed;
    Register->Width = 64;
    Register->Numerator = 1;
    Register->Denominator = 1;
    Register->Accuracy = 0;

    return STATUS_SUCCESS;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveRTStream::GetPosition(PKSAUDIO_POSITION Position) {
    if (!Position) return STATUS_INVALID_PARAMETER;

    ULONG bytePos = GetCurrentPositionBytes();
    Position->PlayOffset = bytePos;
    Position->WriteOffset = bytePos;

    return STATUS_SUCCESS;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveRTStream::RegisterNotificationEvent(PKEVENT NotificationEvent) {
    PAGED_CODE();
    if (!NotificationEvent) return STATUS_INVALID_PARAMETER;

    if (m_NotificationEventsCount < 2) {
        m_pNotificationEvents[m_NotificationEventsCount++] = NotificationEvent;
        return STATUS_SUCCESS;
    }
    return STATUS_INSUFFICIENT_RESOURCES;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveRTStream::UnregisterNotificationEvent(PKEVENT NotificationEvent) {
    PAGED_CODE();
    if (!NotificationEvent) return STATUS_INVALID_PARAMETER;

    for (ULONG i = 0; i < m_NotificationEventsCount; i++) {
        if (m_pNotificationEvents[i] == NotificationEvent) {
            m_pNotificationEvents[i] = m_pNotificationEvents[--m_NotificationEventsCount];
            return STATUS_SUCCESS;
        }
    }
    return STATUS_NOT_FOUND;
}

ULONG CMiniportWaveRTStream::GetCurrentPositionBytes() {
    if (m_State != KSSTATE_RUN || m_DmaBufferSize == 0) {
        return 0;
    }

    if (g_pSharedBridge) {
        m_PositionRegister = g_pSharedBridge->GetCurrentBytePosition(m_PinId);
        return m_PositionRegister;
    }

    LARGE_INTEGER now = KeQueryPerformanceCounter(NULL);
    LONGLONG elapsedQpc = now.QuadPart - m_StartTime.QuadPart;
    if (elapsedQpc < 0) elapsedQpc = 0;

    // Calculate elapsed frames at 48000 Hz, stereo 32-bit float (8 bytes per frame)
    LONGLONG elapsedFrames = (elapsedQpc * OWL_DEFAULT_SAMPLE_RATE) / m_QpcFrequency.QuadPart;
    ULONG byteOffset = (ULONG)((elapsedFrames * OWL_DEFAULT_CHANNELS * sizeof(FLOAT)) % m_DmaBufferSize);

    m_PositionRegister = byteOffset;
    return byteOffset;
}

VOID CMiniportWaveRTStream::UpdatePositionRegister() {
    GetCurrentPositionBytes();
}
