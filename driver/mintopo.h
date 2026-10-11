#pragma once

#ifndef _MINTOPO_H_
#define _MINTOPO_H_

#include "common.h"

class CMiniportTopology : public IMiniportTopology {
private:
    LONG        m_cRef;
    PPORTTOPOLOGY m_pPort;

public:
    CMiniportTopology();
    ~CMiniportTopology();

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

    // IMiniportTopology
    STDMETHODIMP_(NTSTATUS) Init(
        PUNKNOWN        UnknownAdapter,
        PRESOURCELIST   ResourceList,
        PPORTTOPOLOGY   Port
    ) override;
};

#endif // _MINTOPO_H_
