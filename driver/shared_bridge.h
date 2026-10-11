
#pragma once

#ifndef _SHARED_BRIDGE_H_
#define _SHARED_BRIDGE_H_

#include "common.h"

#define PIN_RENDER           0
#define PIN_CAPTURE_MICFX    1
#define PIN_CAPTURE_STREAMIN 2
#define PIN_COUNT            3

#pragma pack(push, 1)
typedef struct _STREAM_PIPE_HEADER {
    ULONG magic;
    ULONG sample_rate;
    ULONG channels;
    ULONG capacity_frames;
    volatile UINT64 write_pos;
    volatile UINT64 read_pos;
    UCHAR _pad[32];
} STREAM_PIPE_HEADER, *PSTREAM_PIPE_HEADER;
#pragma pack(pop)

struct MappedSection {
    PVOID pViewBase;
    SIZE_T ViewSize;
    PMDL pMdl;
    PVOID pSystemAddress;
};

class COwlSharedBridge {
private:
    KSPIN_LOCK          m_Lock;
    
    // Timer and DPC for audio processing
    KTIMER              m_Timer;
    KDPC                m_Dpc;
    BOOLEAN             m_TimerRunning;
    
    // Per-pin state
    PVOID               m_DmaBuffers[PIN_COUNT];
    ULONG               m_DmaBufferSizes[PIN_COUNT];
    volatile ULONG      m_Positions[PIN_COUNT];
    volatile ULONG*     m_pPositionRegisters[PIN_COUNT];
    BOOLEAN             m_PinActive[PIN_COUNT];

    // Shared Memory Mapping
    MappedSection       m_StreamMixMap;
    MappedSection       m_MicFXMap;
    PSTREAM_PIPE_HEADER m_pStreamMixPipe;
    PSTREAM_PIPE_HEADER m_pMicFXPipe;
    
    PETHREAD            m_WorkerThread;
    KEVENT              m_WorkerEvent;
    BOOLEAN             m_WorkerTerminate;
    
    volatile UINT64     m_TotalFrames;

    static NTSTATUS MapSectionSecurely(PCWSTR sectionName, MappedSection* pMap);
    static VOID UnmapSectionSecurely(MappedSection* pMap);

    static VOID TimerDpcRoutine(PKDPC Dpc, PVOID DeferredContext, PVOID SystemArgument1, PVOID SystemArgument2);
    static VOID WorkerThreadStub(PVOID Context);
    VOID WorkerThread();
    VOID ProcessAudio();

public:
    COwlSharedBridge();
    ~COwlSharedBridge();

    NTSTATUS InitializePin(ULONG pinId, PVOID pDmaBuffer, ULONG bufferSize, volatile ULONG* pPositionRegister);
    VOID     CleanupPin(ULONG pinId);
    VOID     SetPinState(ULONG pinId, KSSTATE state);

    // Get current position in bytes for WaveRT Position Register
    ULONG    GetCurrentBytePosition(ULONG pinId);

    // Driver status helper
    VOID     GetStatus(POWL_DRIVER_STATUS pStatus);
};

// Global singleton bridge
extern COwlSharedBridge* g_pSharedBridge;

#endif // _SHARED_BRIDGE_H_

