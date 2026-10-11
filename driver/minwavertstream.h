#pragma once

#ifndef _MINWAVERTSTREAM_H_
#define _MINWAVERTSTREAM_H_

#include "common.h"
#include "shared_bridge.h"

class CMiniportWaveRT;

class CMiniportWaveRTStream : public IMiniportWaveRTStreamNotification {
private:
    LONG                m_cRef;
    CMiniportWaveRT*    m_pMiniport;
    ULONG               m_PinId;
    BOOLEAN             m_bCapture;
    KSSTATE             m_State;
    PKSDATAFORMAT_WAVEFORMATEXTENSIBLE m_pFormat;

    // DMA Cyclic buffer
    PMDL                m_pMdl;
    PVOID               m_pDmaBuffer;
    ULONG               m_DmaBufferSize;

    // WaveRT Position Register (mapped to audiodg.exe)
    ULONG               m_PositionRegister;
    PKSRTAUDIO_HWREGISTER m_pHwPositionRegister;

    // Real-Time Notification events
    PKEVENT             m_pNotificationEvents[2];
    ULONG               m_NotificationEventsCount;

    // Simulation / Timing
    LARGE_INTEGER       m_StartTime;
    LARGE_INTEGER       m_QpcFrequency;
    UINT64              m_TotalFramesProcessed;

public:
    CMiniportWaveRTStream(CMiniportWaveRT* miniport, ULONG pinId, BOOLEAN bCapture);
    ~CMiniportWaveRTStream();

    // IUnknown
    STDMETHODIMP QueryInterface(REFIID InterfaceId, PVOID* Interface) override;
    STDMETHODIMP_(ULONG) AddRef() override;
    STDMETHODIMP_(ULONG) Release() override;

    // IMiniportWaveRTStream
    STDMETHODIMP_(NTSTATUS) SetFormat(PKSDATAFORMAT DataFormat) override;
    STDMETHODIMP_(NTSTATUS) SetState(KSSTATE State) override;
    STDMETHODIMP_(NTSTATUS) GetPosition(PKSAUDIO_POSITION Position) override;
    STDMETHODIMP_(NTSTATUS) AllocateAudioBuffer(
        ULONG               RequestedSize,
        PMDL*               AudioBufferMdl,
        ULONG*              ActualSize,
        ULONG*              OffsetFromLo,
        MEMORY_CACHING_TYPE* CacheType
    ) override;
    STDMETHODIMP_(VOID) FreeAudioBuffer(PMDL AudioBufferMdl, ULONG BufferSize) override;
    STDMETHODIMP_(VOID) GetHardwareLatency(PKSRTAUDIO_HWLATENCY Latency) override;
    STDMETHODIMP_(NTSTATUS) GetPositionRegister(PKSRTAUDIO_HWREGISTER Register) override;
    STDMETHODIMP_(NTSTATUS) GetClockRegister(PKSRTAUDIO_HWREGISTER Register) override;

    // IMiniportWaveRTStreamNotification
    STDMETHODIMP_(NTSTATUS) RegisterNotificationEvent(PKEVENT NotificationEvent) override;
    STDMETHODIMP_(NTSTATUS) UnregisterNotificationEvent(PKEVENT NotificationEvent) override;

    // Internal Stream Helpers
    ULONG GetCurrentPositionBytes();
    VOID  UpdatePositionRegister();
};

#endif // _MINWAVERTSTREAM_H_
