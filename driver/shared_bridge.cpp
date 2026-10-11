
#include "shared_bridge.h"
#include <ntstrsafe.h>

COwlSharedBridge* g_pSharedBridge = NULL;

NTSTATUS COwlSharedBridge::MapSectionSecurely(PCWSTR sectionName, MappedSection* pMap) {
    UNICODE_STRING name;
    OBJECT_ATTRIBUTES oa;
    HANDLE hSection = NULL;
    NTSTATUS status;

    RtlInitUnicodeString(&name, sectionName);
    InitializeObjectAttributes(&oa, &name, OBJ_CASE_INSENSITIVE | OBJ_KERNEL_HANDLE, NULL, NULL);
    status = ZwOpenSection(&hSection, SECTION_MAP_READ | SECTION_MAP_WRITE, &oa);

    if (!NT_SUCCESS(status)) {
        WCHAR sessionNameBuf[256];
        RtlStringCbPrintfW(sessionNameBuf, sizeof(sessionNameBuf), L"\\Sessions\\1%ws", sectionName);
        RtlInitUnicodeString(&name, sessionNameBuf);
        InitializeObjectAttributes(&oa, &name, OBJ_CASE_INSENSITIVE | OBJ_KERNEL_HANDLE, NULL, NULL);
        status = ZwOpenSection(&hSection, SECTION_MAP_READ | SECTION_MAP_WRITE, &oa);
    }

    if (!NT_SUCCESS(status)) {
        WCHAR sessionNameBuf[256];
        RtlStringCbPrintfW(sessionNameBuf, sizeof(sessionNameBuf), L"\\Sessions\\2%ws", sectionName);
        RtlInitUnicodeString(&name, sessionNameBuf);
        InitializeObjectAttributes(&oa, &name, OBJ_CASE_INSENSITIVE | OBJ_KERNEL_HANDLE, NULL, NULL);
        status = ZwOpenSection(&hSection, SECTION_MAP_READ | SECTION_MAP_WRITE, &oa);
    }

    if (!NT_SUCCESS(status)) {
        return status;
    }

    PVOID pViewBase = NULL;
    SIZE_T viewSize = 0;
    
    status = ZwMapViewOfSection(hSection, NtCurrentProcess(), &pViewBase, 0, 0, NULL, &viewSize, ViewUnmap, 0, PAGE_READWRITE);
    ZwClose(hSection);
    
    if (!NT_SUCCESS(status)) {
        return status;
    }

    PMDL pMdl = IoAllocateMdl(pViewBase, (ULONG)viewSize, FALSE, FALSE, NULL);
    if (!pMdl) {
        ZwUnmapViewOfSection(NtCurrentProcess(), pViewBase);
        return STATUS_INSUFFICIENT_RESOURCES;
    }

    __try {
        MmProbeAndLockPages(pMdl, KernelMode, IoModifyAccess);
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        IoFreeMdl(pMdl);
        ZwUnmapViewOfSection(NtCurrentProcess(), pViewBase);
        return GetExceptionCode();
    }

    PVOID pSystemAddress = MmGetSystemAddressForMdlSafe(pMdl, NormalPagePriority | MdlMappingNoExecute);
    if (!pSystemAddress) {
        MmUnlockPages(pMdl);
        IoFreeMdl(pMdl);
        ZwUnmapViewOfSection(NtCurrentProcess(), pViewBase);
        return STATUS_INSUFFICIENT_RESOURCES;
    }

    pMap->pViewBase = pViewBase;
    pMap->ViewSize = viewSize;
    pMap->pMdl = pMdl;
    pMap->pSystemAddress = pSystemAddress;

    return STATUS_SUCCESS;
}

VOID COwlSharedBridge::UnmapSectionSecurely(MappedSection* pMap) {
    if (pMap->pMdl) {
        MmUnlockPages(pMap->pMdl);
        IoFreeMdl(pMap->pMdl);
        pMap->pMdl = NULL;
    }
    if (pMap->pViewBase) {
        ZwUnmapViewOfSection(NtCurrentProcess(), pMap->pViewBase);
        pMap->pViewBase = NULL;
    }
    pMap->pSystemAddress = NULL;
    pMap->ViewSize = 0;
}

COwlSharedBridge::COwlSharedBridge()
    : m_TimerRunning(FALSE),
      m_pStreamMixPipe(NULL),
      m_pMicFXPipe(NULL),
      m_TotalFrames(0),
      m_WorkerThread(NULL)
{
    KeInitializeSpinLock(&m_Lock);
    
    RtlZeroMemory(&m_StreamMixMap, sizeof(m_StreamMixMap));
    RtlZeroMemory(&m_MicFXMap, sizeof(m_MicFXMap));

    for (int i = 0; i < PIN_COUNT; i++) {
        m_DmaBuffers[i] = NULL;
        m_DmaBufferSizes[i] = 0;
        m_Positions[i] = 0;
        m_pPositionRegisters[i] = NULL;
        m_PinActive[i] = FALSE;
    }

    KeInitializeDpc(&m_Dpc, TimerDpcRoutine, this);
    KeInitializeTimer(&m_Timer);
    
    KeInitializeEvent(&m_WorkerEvent, SynchronizationEvent, FALSE);
    m_WorkerTerminate = FALSE;
    
    HANDLE hThread;
    if (NT_SUCCESS(PsCreateSystemThread(&hThread, THREAD_ALL_ACCESS, NULL, NULL, NULL, WorkerThreadStub, this))) {
        ObReferenceObjectByHandle(hThread, THREAD_ALL_ACCESS, NULL, KernelMode, (PVOID*)&m_WorkerThread, NULL);
        ZwClose(hThread);
    }
}

COwlSharedBridge::~COwlSharedBridge() {
    if (m_TimerRunning) {
        KeCancelTimer(&m_Timer);
        m_TimerRunning = FALSE;
    }
    
    m_WorkerTerminate = TRUE;
    KeSetEvent(&m_WorkerEvent, 0, FALSE);
    if (m_WorkerThread) {
        KeWaitForSingleObject(m_WorkerThread, Executive, KernelMode, FALSE, NULL);
        ObDereferenceObject(m_WorkerThread);
        m_WorkerThread = NULL;
    }
}

VOID COwlSharedBridge::WorkerThreadStub(PVOID Context) {
    if (Context) ((COwlSharedBridge*)Context)->WorkerThread();
}

VOID COwlSharedBridge::WorkerThread() {
    while (!m_WorkerTerminate) {
        LARGE_INTEGER timeout;
        timeout.QuadPart = -10000000; // 1 second
        KeWaitForSingleObject(&m_WorkerEvent, Executive, KernelMode, FALSE, &timeout);
        
        if (m_WorkerTerminate) break;
        
        if (!m_StreamMixMap.pSystemAddress) {
            if (NT_SUCCESS(MapSectionSecurely(L"\\BaseNamedObjects\\OpenWaveLink_StreamMix_PCM", &m_StreamMixMap))) {
                KLOCK_QUEUE_HANDLE lockHandle;
                KeAcquireInStackQueuedSpinLock(&m_Lock, &lockHandle);
                m_pStreamMixPipe = (PSTREAM_PIPE_HEADER)m_StreamMixMap.pSystemAddress;
                KeReleaseInStackQueuedSpinLock(&lockHandle);
            }
        }
        
        if (!m_MicFXMap.pSystemAddress) {
            if (NT_SUCCESS(MapSectionSecurely(L"\\BaseNamedObjects\\OpenWaveLink_MicFX_PCM", &m_MicFXMap))) {
                KLOCK_QUEUE_HANDLE lockHandle;
                KeAcquireInStackQueuedSpinLock(&m_Lock, &lockHandle);
                m_pMicFXPipe = (PSTREAM_PIPE_HEADER)m_MicFXMap.pSystemAddress;
                KeReleaseInStackQueuedSpinLock(&lockHandle);
            }
        }
    }
    
    KLOCK_QUEUE_HANDLE lockHandle;
    KeAcquireInStackQueuedSpinLock(&m_Lock, &lockHandle);
    m_pStreamMixPipe = NULL;
    m_pMicFXPipe = NULL;
    KeReleaseInStackQueuedSpinLock(&lockHandle);
    
    UnmapSectionSecurely(&m_StreamMixMap);
    UnmapSectionSecurely(&m_MicFXMap);
    
    PsTerminateSystemThread(STATUS_SUCCESS);
}

NTSTATUS COwlSharedBridge::InitializePin(ULONG pinId, PVOID pDmaBuffer, ULONG bufferSize, volatile ULONG* pPositionRegister) {
    if (pinId >= PIN_COUNT) return STATUS_INVALID_PARAMETER;

    KLOCK_QUEUE_HANDLE lockHandle;
    KeAcquireInStackQueuedSpinLock(&m_Lock, &lockHandle);

    m_DmaBuffers[pinId] = pDmaBuffer;
    m_DmaBufferSizes[pinId] = bufferSize;
    m_Positions[pinId] = 0;
    m_pPositionRegisters[pinId] = pPositionRegister;
    m_PinActive[pinId] = FALSE;

    KeReleaseInStackQueuedSpinLock(&lockHandle);
    return STATUS_SUCCESS;
}

VOID COwlSharedBridge::CleanupPin(ULONG pinId) {
    if (pinId >= PIN_COUNT) return;

    KLOCK_QUEUE_HANDLE lockHandle;
    KeAcquireInStackQueuedSpinLock(&m_Lock, &lockHandle);

    m_DmaBuffers[pinId] = NULL;
    m_DmaBufferSizes[pinId] = 0;
    m_Positions[pinId] = 0;
    m_pPositionRegisters[pinId] = NULL;
    m_PinActive[pinId] = FALSE;
    
    BOOLEAN anyActive = FALSE;
    for (int i = 0; i < PIN_COUNT; i++) {
        if (m_PinActive[i]) anyActive = TRUE;
    }
    if (!anyActive && m_TimerRunning) {
        KeCancelTimer(&m_Timer);
        m_TimerRunning = FALSE;
    }

    KeReleaseInStackQueuedSpinLock(&lockHandle);
}

VOID COwlSharedBridge::SetPinState(ULONG pinId, KSSTATE state) {
    if (pinId >= PIN_COUNT) return;

    KLOCK_QUEUE_HANDLE lockHandle;
    KeAcquireInStackQueuedSpinLock(&m_Lock, &lockHandle);

    if (state == KSSTATE_RUN) {
        m_PinActive[pinId] = TRUE;
        
        if (!m_TimerRunning) {
            LARGE_INTEGER dueTime;
            dueTime.QuadPart = -10000; // 1 ms
            KeSetTimerEx(&m_Timer, dueTime, 1, &m_Dpc);
            m_TimerRunning = TRUE;
        }
    } else {
        m_PinActive[pinId] = FALSE;
        
        BOOLEAN anyActive = FALSE;
        for (int i = 0; i < PIN_COUNT; i++) {
            if (m_PinActive[i]) anyActive = TRUE;
        }
        if (!anyActive && m_TimerRunning) {
            KeCancelTimer(&m_Timer);
            m_TimerRunning = FALSE;
        }
    }

    KeReleaseInStackQueuedSpinLock(&lockHandle);
}

VOID COwlSharedBridge::TimerDpcRoutine(PKDPC Dpc, PVOID DeferredContext, PVOID SystemArgument1, PVOID SystemArgument2) {
    UNREFERENCED_PARAMETER(Dpc);
    UNREFERENCED_PARAMETER(SystemArgument1);
    UNREFERENCED_PARAMETER(SystemArgument2);

    COwlSharedBridge* bridge = (COwlSharedBridge*)DeferredContext;
    if (bridge) {
        bridge->ProcessAudio();
    }
}

VOID COwlSharedBridge::ProcessAudio() {
    KLOCK_QUEUE_HANDLE lockHandle;
    KeAcquireInStackQueuedSpinLockAtDpcLevel(&m_Lock, &lockHandle);

    ULONG framesToProcess = 48; 
    
    struct PinConfig {
        ULONG id;
        PSTREAM_PIPE_HEADER pipe;
        SIZE_T viewSize;
    };
    
    PinConfig captures[] = {
        { PIN_CAPTURE_STREAMIN, m_pStreamMixPipe, m_StreamMixMap.ViewSize },
        { PIN_CAPTURE_MICFX, m_pMicFXPipe, m_MicFXMap.ViewSize }
    };

    for (int idx = 0; idx < 2; idx++) {
        ULONG pinId = captures[idx].id;
        PSTREAM_PIPE_HEADER pipe = captures[idx].pipe;
        SIZE_T viewSize = captures[idx].viewSize;
        
        if (m_PinActive[pinId] && m_DmaBuffers[pinId] && m_DmaBufferSizes[pinId] > 0) {
            ULONG dmaFrames = m_DmaBufferSizes[pinId] / (OWL_DEFAULT_CHANNELS * sizeof(FLOAT));
            ULONG currentDmaRead = m_Positions[pinId] / (OWL_DEFAULT_CHANNELS * sizeof(FLOAT));
            FLOAT* pDstData = (FLOAT*)m_DmaBuffers[pinId];
            
            if (pipe && viewSize >= sizeof(STREAM_PIPE_HEADER) && pipe->magic == 0x50434D31) {
                ULONG capacity = pipe->capacity_frames;
                if (capacity > 0 && (capacity & (capacity - 1)) == 0 &&
                    viewSize >= sizeof(STREAM_PIPE_HEADER) + (capacity * OWL_DEFAULT_CHANNELS * sizeof(FLOAT))) {
                    
                    UINT64 writePos = pipe->write_pos;
                    UINT64 readPos = pipe->read_pos;
                    
                    ULONG available = (ULONG)(writePos - readPos);
                    ULONG toRead = available > framesToProcess ? framesToProcess : available;
                    
                    if (toRead > 0) {
                        FLOAT* pSrcData = (FLOAT*)(pipe + 1);
                        
                        for (ULONG i = 0; i < toRead; i++) {
                            ULONG srcIdx = (readPos + i) & (capacity - 1);
                            ULONG dstIdx = (currentDmaRead + i) % dmaFrames;
                            
                            pDstData[dstIdx * 2] = pSrcData[srcIdx * 2];
                            pDstData[dstIdx * 2 + 1] = pSrcData[srcIdx * 2 + 1];
                        }
                        
                        pipe->read_pos += toRead;
                        m_Positions[pinId] = ((currentDmaRead + toRead) % dmaFrames) * OWL_DEFAULT_CHANNELS * sizeof(FLOAT);
                    }
                }
            } else {
                for (ULONG i = 0; i < framesToProcess; i++) {
                    ULONG dstIdx = (currentDmaRead + i) % dmaFrames;
                    pDstData[dstIdx * 2] = 0.0f;
                    pDstData[dstIdx * 2 + 1] = 0.0f;
                }
                m_Positions[pinId] = ((currentDmaRead + framesToProcess) % dmaFrames) * OWL_DEFAULT_CHANNELS * sizeof(FLOAT);
            }
            if (m_pPositionRegisters[pinId]) {
                *(m_pPositionRegisters[pinId]) = m_Positions[pinId];
            }
        }
    }
    
    if (m_PinActive[PIN_RENDER] && m_DmaBuffers[PIN_RENDER] && m_DmaBufferSizes[PIN_RENDER] > 0) {
        ULONG dmaFrames = m_DmaBufferSizes[PIN_RENDER] / (OWL_DEFAULT_CHANNELS * sizeof(FLOAT));
        ULONG currentDmaWrite = m_Positions[PIN_RENDER] / (OWL_DEFAULT_CHANNELS * sizeof(FLOAT));
        
        m_Positions[PIN_RENDER] = ((currentDmaWrite + framesToProcess) % dmaFrames) * OWL_DEFAULT_CHANNELS * sizeof(FLOAT);
        if (m_pPositionRegisters[PIN_RENDER]) {
            *(m_pPositionRegisters[PIN_RENDER]) = m_Positions[PIN_RENDER];
        }
    }
    
    m_TotalFrames += framesToProcess;

    KeReleaseInStackQueuedSpinLockFromDpcLevel(&lockHandle);
}

ULONG COwlSharedBridge::GetCurrentBytePosition(ULONG pinId) {
    if (pinId >= PIN_COUNT) return 0;
    return m_Positions[pinId];
}

VOID COwlSharedBridge::GetStatus(POWL_DRIVER_STATUS pStatus) {
    if (!pStatus) return;

    RtlZeroMemory(pStatus, sizeof(OWL_DRIVER_STATUS));
    pStatus->DriverVersionMajor = 1;
    pStatus->DriverVersionMinor = 0;
    
    ULONG active = 0;
    for (int i = 0; i < PIN_COUNT; i++) {
        if (m_PinActive[i]) active++;
    }
    
    pStatus->ActiveStreamsCount = active;
    pStatus->RenderActive = m_PinActive[PIN_RENDER] ? 1 : 0;
    pStatus->MicFXActive = m_PinActive[PIN_CAPTURE_MICFX] ? 1 : 0;
    pStatus->StreamInActive = m_PinActive[PIN_CAPTURE_STREAMIN] ? 1 : 0;
    pStatus->TotalFramesTransferred = m_TotalFrames;
    pStatus->SampleRate = OWL_DEFAULT_SAMPLE_RATE;
    pStatus->BufferSizeBytes = OWL_MAX_BUFFER_SIZE;
}

