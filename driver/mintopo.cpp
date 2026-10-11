#include "mintopo.h"

#pragma code_seg("PAGE")

static PCPIN_DESCRIPTOR TopologyPins[] = {
    // Pin 0: KSPIN_DATAFLOW_IN (Render Stream)
    {
        0, 0, 0,
        NULL,
        {
            0, NULL,
            0, NULL,
            0, NULL,
            KSPIN_DATAFLOW_IN,
            KSPIN_COMMUNICATION_NONE,
            &KSCATEGORY_AUDIO,
            NULL,
            0
        }
    },
    // Pin 1: KSPIN_DATAFLOW_OUT (Bridge/Speaker Out)
    {
        0, 0, 0,
        NULL,
        {
            0, NULL,
            0, NULL,
            0, NULL,
            KSPIN_DATAFLOW_OUT,
            KSPIN_COMMUNICATION_NONE,
            &KSCATEGORY_AUDIO,
            NULL,
            0
        }
    },
    // Pin 2: KSPIN_DATAFLOW_IN (Microphone Bridge In)
    {
        0, 0, 0,
        NULL,
        {
            0, NULL,
            0, NULL,
            0, NULL,
            KSPIN_DATAFLOW_IN,
            KSPIN_COMMUNICATION_NONE,
            &KSCATEGORY_AUDIO,
            NULL,
            0
        }
    },
    // Pin 3: KSPIN_DATAFLOW_OUT (Capture Stream)
    {
        0, 0, 0,
        NULL,
        {
            0, NULL,
            0, NULL,
            0, NULL,
            KSPIN_DATAFLOW_OUT,
            KSPIN_COMMUNICATION_NONE,
            &KSCATEGORY_AUDIO,
            NULL,
            0
        }
    }
};

static GUID TopologyCategories[] = {
    STATICGUIDOF(KSCATEGORY_TOPOLOGY)
};

static PCFILTER_DESCRIPTOR TopologyFilterDescriptor = {
    0,                                  // Version
    NULL,                               // AutomationTable
    sizeof(PCPIN_DESCRIPTOR),           // PinSize
    SIZEOF_ARRAY(TopologyPins),         // PinCount
    TopologyPins,                       // Pins
    sizeof(PCNODE_DESCRIPTOR),          // NodeSize
    0,                                  // NodeCount
    NULL,                               // Nodes
    0,                                  // ConnectionCount
    NULL,                               // Connections
    SIZEOF_ARRAY(TopologyCategories),   // CategoryCount
    TopologyCategories                  // Categories
};

CMiniportTopology::CMiniportTopology()
    : m_cRef(1),
      m_pPort(NULL)
{
}

CMiniportTopology::~CMiniportTopology() {
    if (m_pPort) {
        m_pPort->Release();
        m_pPort = NULL;
    }
}

STDMETHODIMP CMiniportTopology::QueryInterface(REFIID InterfaceId, PVOID* Interface) {
    if (!Interface) return STATUS_INVALID_PARAMETER;

    if (IsEqualGUID(InterfaceId, IID_IUnknown) ||
        IsEqualGUID(InterfaceId, IID_IMiniport) ||
        IsEqualGUID(InterfaceId, IID_IMiniportTopology)) {
        *Interface = static_cast<PVOID>(static_cast<IMiniportTopology*>(this));
        AddRef();
        return STATUS_SUCCESS;
    }

    *Interface = NULL;
    return STATUS_NOINTERFACE;
}

STDMETHODIMP_(ULONG) CMiniportTopology::AddRef() {
    return InterlockedIncrement(&m_cRef);
}

STDMETHODIMP_(ULONG) CMiniportTopology::Release() {
    LONG count = InterlockedDecrement(&m_cRef);
    if (count == 0) {
        delete this;
    }
    return count;
}

STDMETHODIMP_(NTSTATUS) CMiniportTopology::Init(
    PUNKNOWN        UnknownAdapter,
    PRESOURCELIST   ResourceList,
    PPORTTOPOLOGY   Port
) {
    PAGED_CODE();
    UNREFERENCED_PARAMETER(UnknownAdapter);
    UNREFERENCED_PARAMETER(ResourceList);

    if (!Port) return STATUS_INVALID_PARAMETER;

    m_pPort = Port;
    m_pPort->AddRef();

    return STATUS_SUCCESS;
}

STDMETHODIMP_(NTSTATUS) CMiniportTopology::GetDescription(PPCFILTER_DESCRIPTOR* Description) {
    PAGED_CODE();
    if (!Description) return STATUS_INVALID_PARAMETER;

    *Description = &TopologyFilterDescriptor;
    return STATUS_SUCCESS;
}

STDMETHODIMP_(NTSTATUS) CMiniportTopology::DataRangeIntersection(
    ULONG           PinId,
    PKSDATARANGE    DataRange,
    PKSDATARANGE    MatchingDataRange,
    ULONG           OutputBufferLength,
    PVOID           ResultantFormat,
    PULONG          ResultantFormatLength
) {
    PAGED_CODE();
    UNREFERENCED_PARAMETER(PinId);
    UNREFERENCED_PARAMETER(DataRange);
    UNREFERENCED_PARAMETER(MatchingDataRange);
    UNREFERENCED_PARAMETER(OutputBufferLength);
    UNREFERENCED_PARAMETER(ResultantFormat);
    UNREFERENCED_PARAMETER(ResultantFormatLength);

    return STATUS_NOT_IMPLEMENTED;
}
