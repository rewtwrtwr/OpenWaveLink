#pragma once

#ifndef _MINWAVERT_H_
#define _MINWAVERT_H_

#include "common.h"
#include "minwavertstream.h"

#define PIN_RENDER           0
#define PIN_CAPTURE_MICFX    1
#define PIN_CAPTURE_STREAMIN 2
#define PIN_COUNT            3

class CMiniportWaveRT : public IMiniportWaveRT {
private:
    LONG            m_cRef;
    PPORTWAVERT     m_pPort;
    PDEVICE_OBJECT  m_pDeviceObject;

public:
    CMiniportWaveRT();
    ~CMiniportWaveRT();

    // IUnknown
    STDMETHODIMP QueryInterface(REFIID InterfaceId, PVOID* Interface) override;
    STDMETHODIMP_(ULONG) AddRef() override;
    STDMETHODIMP_(ULONG) Release() override;

    // IMiniport
    STDMETHODIMP_(NTSTATUS) GetDescription(PPCFILTER_DESCRIPTOR* Description) override;
    STDMETHODIMP_(NTSTATUS) DataRangeIntersection(
        ULONG           PinId,
        PKSDATARANGE    DataRange,
        PKSDATARANGE    MatchingDataRange,
        ULONG           OutputBufferLength,
        PVOID           ResultantFormat,
        PULONG          ResultantFormatLength
    ) override;

    // IMiniportWaveRT
    STDMETHODIMP_(NTSTATUS) Init(
        PUNKNOWN        UnknownAdapter,
        PRESOURCELIST   ResourceList,
        PPORTWAVERT     Port
    ) override;
    STDMETHODIMP_(NTSTATUS) NewStream(
        PMINIPORTWAVERTSTREAM*  Stream,
        PPORTWAVERTSTREAM       PortStream,
        ULONG                   Pin,
        BOOLEAN                 Capture,
        PKSDATAFORMAT           DataFormat
    ) override;
    STDMETHODIMP_(NTSTATUS) GetDeviceDescription(PDEVICE_DESCRIPTION DeviceDescription) override;
};

#endif // _MINWAVERT_H_
