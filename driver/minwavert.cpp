#include "minwavert.h"

#pragma code_seg("PAGE")

// Supported Audio Formats: 48 kHz, Stereo, 32-bit Float & PCM
static KSDATARANGE_AUDIO PinFormatRanges[] = {
    // 32-bit Float Range
    {
        {
            sizeof(KSDATARANGE_AUDIO),
            0,
            0,
            0,
            STATICGUIDOF(KSDATAFORMAT_TYPE_AUDIO),
            STATICGUIDOF(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT),
            STATICGUIDOF(KSDATAFORMAT_SPECIFIER_WAVEFORMATEX)
        },
        2,      // Max Channels
        32,     // Min Bits
        32,     // Max Bits
        48000,  // Min Sample Rate
        48000   // Max Sample Rate
    },
    // 16/24-bit PCM Range for backward compatibility
    {
        {
            sizeof(KSDATARANGE_AUDIO),
            0,
            0,
            0,
            STATICGUIDOF(KSDATAFORMAT_TYPE_AUDIO),
            STATICGUIDOF(KSDATAFORMAT_SUBTYPE_PCM),
            STATICGUIDOF(KSDATAFORMAT_SPECIFIER_WAVEFORMATEX)
        },
        2,
        16,
        24,
        48000,
        48000
    }
};

static PKSDATARANGE PinDataRangePointers[] = {
    (PKSDATARANGE)&PinFormatRanges[0],
    (PKSDATARANGE)&PinFormatRanges[1]
};

// WaveRT Pin Descriptors
static PCPIN_DESCRIPTOR MiniportPins[PIN_COUNT] = {
    // Pin 0: OpenWaveLink Out (Stream Mix) - Render Sink
    {
        0, 0, 0,
        NULL,
        {
            0, NULL,
            0, NULL,
            SIZEOF_ARRAY(PinDataRangePointers),
            PinDataRangePointers,
            KSPIN_DATAFLOW_IN,
            KSPIN_COMMUNICATION_SINK,
            &KSCATEGORY_AUDIO,
            NULL,
            0
        }
    },
    // Pin 1: OpenWaveLink MicFX (Processed Voice) - Capture Source
    {
        0, 0, 0,
        NULL,
        {
            0, NULL,
            0, NULL,
            SIZEOF_ARRAY(PinDataRangePointers),
            PinDataRangePointers,
            KSPIN_DATAFLOW_OUT,
            KSPIN_COMMUNICATION_SINK,
            &KSCATEGORY_AUDIO,
            NULL,
            0
        }
    },
    // Pin 2: OpenWaveLink Stream In - Capture Source
    {
        0, 0, 0,
        NULL,
        {
            0, NULL,
            0, NULL,
            SIZEOF_ARRAY(PinDataRangePointers),
            PinDataRangePointers,
            KSPIN_DATAFLOW_OUT,
            KSPIN_COMMUNICATION_SINK,
            &KSCATEGORY_AUDIO,
            NULL,
            0
        }
    }
};

static GUID WaveRTCategories[] = {
    STATICGUIDOF(KSCATEGORY_AUDIO),
    STATICGUIDOF(KSCATEGORY_RENDER),
    STATICGUIDOF(KSCATEGORY_CAPTURE)
};

static PCFILTER_DESCRIPTOR MiniportFilterDescriptor = {
    0,                                  // Version
    NULL,                               // AutomationTable
    sizeof(PCPIN_DESCRIPTOR),           // PinSize
    SIZEOF_ARRAY(MiniportPins),         // PinCount
    MiniportPins,                       // Pins
    sizeof(PCNODE_DESCRIPTOR),          // NodeSize
    0,                                  // NodeCount
    NULL,                               // Nodes
    0,                                  // ConnectionCount
    NULL,                               // Connections
    SIZEOF_ARRAY(WaveRTCategories),     // CategoryCount
    WaveRTCategories                    // Categories
};

CMiniportWaveRT::CMiniportWaveRT()
    : m_cRef(1),
      m_pPort(NULL),
      m_pDeviceObject(NULL)
{
}

CMiniportWaveRT::~CMiniportWaveRT() {
    if (m_pPort) {
        m_pPort->Release();
        m_pPort = NULL;
    }
}

// IUnknown
STDMETHODIMP CMiniportWaveRT::QueryInterface(REFIID InterfaceId, PVOID* Interface) {
    if (!Interface) return STATUS_INVALID_PARAMETER;

    if (IsEqualGUID(InterfaceId, IID_IUnknown) ||
        IsEqualGUID(InterfaceId, IID_IMiniport) ||
        IsEqualGUID(InterfaceId, IID_IMiniportWaveRT)) {
        *Interface = static_cast<PVOID>(static_cast<IMiniportWaveRT*>(this));
        AddRef();
        return STATUS_SUCCESS;
    }

    *Interface = NULL;
    return STATUS_NOINTERFACE;
}

STDMETHODIMP_(ULONG) CMiniportWaveRT::AddRef() {
    return InterlockedIncrement(&m_cRef);
}

STDMETHODIMP_(ULONG) CMiniportWaveRT::Release() {
    LONG count = InterlockedDecrement(&m_cRef);
    if (count == 0) {
        delete this;
    }
    return count;
}

// IMiniport
STDMETHODIMP_(NTSTATUS) CMiniportWaveRT::Init(
    PUNKNOWN        UnknownAdapter,
    PRESOURCELIST   ResourceList,
    PPORTWAVERT     Port
) {
    PAGED_CODE();
    UNREFERENCED_PARAMETER(UnknownAdapter);
    UNREFERENCED_PARAMETER(ResourceList);

    if (!Port) return STATUS_INVALID_PARAMETER;

    m_pPort = Port;
    m_pPort->AddRef();

    return STATUS_SUCCESS;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveRT::GetDescription(PPCFILTER_DESCRIPTOR* Description) {
    PAGED_CODE();
    if (!Description) return STATUS_INVALID_PARAMETER;

    *Description = &MiniportFilterDescriptor;
    return STATUS_SUCCESS;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveRT::DataRangeIntersection(
    ULONG           PinId,
    PKSDATARANGE    DataRange,
    PKSDATARANGE    MatchingDataRange,
    ULONG           OutputBufferLength,
    PVOID           ResultantFormat,
    PULONG          ResultantFormatLength
) {
    PAGED_CODE();
    if (PinId >= PIN_COUNT || !DataRange || !MatchingDataRange || !ResultantFormatLength) {
        return STATUS_INVALID_PARAMETER;
    }

    const ULONG formatSize = sizeof(KSDATAFORMAT_WAVEFORMATEX);
    *ResultantFormatLength = formatSize;

    if (OutputBufferLength == 0) {
        return STATUS_BUFFER_OVERFLOW;
    }

    if (OutputBufferLength < formatSize) {
        return STATUS_BUFFER_TOO_SMALL;
    }

    // Return negotiated 48 kHz stereo 32-bit float format
    PKSDATAFORMAT_WAVEFORMATEX pWfx = (PKSDATAFORMAT_WAVEFORMATEX)ResultantFormat;
    pWfx->DataFormat.FormatSize = formatSize;
    pWfx->DataFormat.Flags = 0;
    pWfx->DataFormat.SampleSize = 8;
    pWfx->DataFormat.Reserved = 0;
    pWfx->DataFormat.MajorFormat = KSDATAFORMAT_TYPE_AUDIO;
    pWfx->DataFormat.SubFormat = KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
    pWfx->DataFormat.Specifier = KSDATAFORMAT_SPECIFIER_WAVEFORMATEX;

    pWfx->WaveFormatEx.wFormatTag = WAVE_FORMAT_IEEE_FLOAT;
    pWfx->WaveFormatEx.nChannels = OWL_DEFAULT_CHANNELS;
    pWfx->WaveFormatEx.nSamplesPerSec = OWL_DEFAULT_SAMPLE_RATE;
    pWfx->WaveFormatEx.nAvgBytesPerSec = OWL_DEFAULT_SAMPLE_RATE * OWL_DEFAULT_CHANNELS * sizeof(FLOAT);
    pWfx->WaveFormatEx.nBlockAlign = OWL_DEFAULT_CHANNELS * sizeof(FLOAT);
    pWfx->WaveFormatEx.wBitsPerSample = 32;
    pWfx->WaveFormatEx.cbSize = 0;

    return STATUS_SUCCESS;
}

// IMiniportWaveRT
STDMETHODIMP_(NTSTATUS) CMiniportWaveRT::NewStream(
    PMINIPORTWAVERTSTREAM*  Stream,
    PPORTWAVERTSTREAM       PortStream,
    ULONG                   Pin,
    BOOLEAN                 Capture,
    PKSDATAFORMAT           DataFormat
) {
    PAGED_CODE();
    UNREFERENCED_PARAMETER(PortStream);

    if (!Stream || Pin >= PIN_COUNT) {
        return STATUS_INVALID_PARAMETER;
    }

    CMiniportWaveRTStream* pStream = new (NonPagedPoolNx, OWL_POOL_TAG)
        CMiniportWaveRTStream(this, Pin, Capture);

    if (!pStream) {
        return STATUS_INSUFFICIENT_RESOURCES;
    }

    NTSTATUS ntStatus = pStream->SetFormat(DataFormat);
    if (!NT_SUCCESS(ntStatus)) {
        pStream->Release();
        return ntStatus;
    }

    *Stream = static_cast<PMINIPORTWAVERTSTREAM>(pStream);
    return STATUS_SUCCESS;
}

STDMETHODIMP_(NTSTATUS) CMiniportWaveRT::GetDeviceDescription(PDEVICE_DESCRIPTION DeviceDescription) {
    PAGED_CODE();
    if (!DeviceDescription) return STATUS_INVALID_PARAMETER;

    RtlZeroMemory(DeviceDescription, sizeof(DEVICE_DESCRIPTION));
    DeviceDescription->Master = TRUE;
    DeviceDescription->ScatterGather = TRUE;
    DeviceDescription->Dma32BitAddresses = TRUE;
    DeviceDescription->InterfaceType = Internal;
    DeviceDescription->MaximumLength = OWL_MAX_BUFFER_SIZE;

    return STATUS_SUCCESS;
}
