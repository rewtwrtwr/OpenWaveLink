#include "adapter.h"

#pragma code_seg("INIT")

extern "C" NTSTATUS DriverEntry(
    PDRIVER_OBJECT  DriverObject,
    PUNICODE_STRING RegistryPath
) {
    PAGED_CODE();

    KdPrintEx((DPFLTR_IHVDRIVER_ID, DPFLTR_INFO_LEVEL, "[OpenWaveLinkAudio] DriverEntry start\n"));

    // Allocate and initialize global shared memory audio bridge
    g_pSharedBridge = new (NonPagedPoolNx, OWL_POOL_TAG) COwlSharedBridge();
    if (!g_pSharedBridge) {
        return STATUS_INSUFFICIENT_RESOURCES;
    }

    // Initialize PortCls driver object
    NTSTATUS ntStatus = PcInitializeAdapterDriver(
        DriverObject,
        RegistryPath,
        AddDevice
    );

    if (!NT_SUCCESS(ntStatus)) {
        delete g_pSharedBridge;
        g_pSharedBridge = NULL;
        return ntStatus;
    }

    // Hook IRP_MJ_DEVICE_CONTROL for user-mode owl-core IOCTL communication
    DriverObject->MajorFunction[IRP_MJ_CREATE]         = OwlDispatchCreate;
    DriverObject->MajorFunction[IRP_MJ_CLOSE]          = OwlDispatchClose;
    DriverObject->MajorFunction[IRP_MJ_DEVICE_CONTROL] = OwlDispatchDeviceControl;
    DriverObject->DriverUnload                         = DriverUnload;

    KdPrintEx((DPFLTR_IHVDRIVER_ID, DPFLTR_INFO_LEVEL, "[OpenWaveLinkAudio] DriverEntry success\n"));
    return STATUS_SUCCESS;
}

#pragma code_seg("PAGE")

VOID DriverUnload(PDRIVER_OBJECT DriverObject) {
    PAGED_CODE();
    UNREFERENCED_PARAMETER(DriverObject);

    UNICODE_STRING dosDeviceName;
    RtlInitUnicodeString(&dosDeviceName, OWL_DOS_DEVICE_NAME);
    IoDeleteSymbolicLink(&dosDeviceName);

    if (g_pSharedBridge) {
        delete g_pSharedBridge;
        g_pSharedBridge = NULL;
    }

    KdPrintEx((DPFLTR_IHVDRIVER_ID, DPFLTR_INFO_LEVEL, "[OpenWaveLinkAudio] DriverUnload completed\n"));
}

NTSTATUS AddDevice(
    PDRIVER_OBJECT DriverObject,
    PDEVICE_OBJECT PhysicalDeviceObject
) {
    PAGED_CODE();

    KdPrintEx((DPFLTR_IHVDRIVER_ID, DPFLTR_INFO_LEVEL, "[OpenWaveLinkAudio] AddDevice start\n"));

    PDEVICE_OBJECT functionalDeviceObject = NULL;
    NTSTATUS ntStatus = PcAddAdapterDevice(
        DriverObject,
        PhysicalDeviceObject,
        PCPFNSTARTDEVICE(NULL),
        OWL_MAX_BUFFER_SIZE,
        0
    );

    if (!NT_SUCCESS(ntStatus)) {
        return ntStatus;
    }

    functionalDeviceObject = DriverObject->DeviceObject;

    // Create DOS symbolic link for User-Mode IOCTL access
    UNICODE_STRING deviceName;
    UNICODE_STRING dosDeviceName;
    RtlInitUnicodeString(&deviceName, OWL_DEVICE_NAME);
    RtlInitUnicodeString(&dosDeviceName, OWL_DOS_DEVICE_NAME);

    IoCreateSymbolicLink(&dosDeviceName, &deviceName);

    // Register WaveRT Port & Miniport
    PPORTWAVERT pPortWaveRT = NULL;
    ntStatus = PcNewPort((PPORT*)&pPortWaveRT, CLSID_PortWaveRT);
    if (NT_SUCCESS(ntStatus)) {
        CMiniportWaveRT* pMiniportWaveRT = new (NonPagedPoolNx, OWL_POOL_TAG) CMiniportWaveRT();
        if (pMiniportWaveRT) {
            ntStatus = pPortWaveRT->Init(functionalDeviceObject, NULL, pMiniportWaveRT, NULL);
            if (NT_SUCCESS(ntStatus)) {
                // Register WaveRender subdevice
                PcRegisterSubdevice(functionalDeviceObject, L"WaveRender", pPortWaveRT);
                // Register WaveMicFX subdevice
                PcRegisterSubdevice(functionalDeviceObject, L"WaveMicFX", pPortWaveRT);
                // Register WaveStreamIn subdevice
                PcRegisterSubdevice(functionalDeviceObject, L"WaveStreamIn", pPortWaveRT);
            }
            pMiniportWaveRT->Release();
        }
        pPortWaveRT->Release();
    }

    // Register Topology Port & Miniport
    PPORTTOPOLOGY pPortTopology = NULL;
    ntStatus = PcNewPort((PPORT*)&pPortTopology, CLSID_PortTopology);
    if (NT_SUCCESS(ntStatus)) {
        CMiniportTopology* pMiniportTopology = new (NonPagedPoolNx, OWL_POOL_TAG) CMiniportTopology();
        if (pMiniportTopology) {
            ntStatus = pPortTopology->Init(functionalDeviceObject, NULL, pMiniportTopology, NULL);
            if (NT_SUCCESS(ntStatus)) {
                PcRegisterSubdevice(functionalDeviceObject, L"Topology", pPortTopology);
            }
            pMiniportTopology->Release();
        }
        pPortTopology->Release();
    }

    KdPrintEx((DPFLTR_IHVDRIVER_ID, DPFLTR_INFO_LEVEL, "[OpenWaveLinkAudio] AddDevice finished\n"));
    return STATUS_SUCCESS;
}

NTSTATUS OwlDispatchCreate(PDEVICE_OBJECT DeviceObject, PIRP Irp) {
    PAGED_CODE();
    UNREFERENCED_PARAMETER(DeviceObject);

    Irp->IoStatus.Status = STATUS_SUCCESS;
    Irp->IoStatus.Information = 0;
    IoCompleteRequest(Irp, IO_NO_INCREMENT);
    return STATUS_SUCCESS;
}

NTSTATUS OwlDispatchClose(PDEVICE_OBJECT DeviceObject, PIRP Irp) {
    PAGED_CODE();
    UNREFERENCED_PARAMETER(DeviceObject);

    Irp->IoStatus.Status = STATUS_SUCCESS;
    Irp->IoStatus.Information = 0;
    IoCompleteRequest(Irp, IO_NO_INCREMENT);
    return STATUS_SUCCESS;
}

NTSTATUS OwlDispatchDeviceControl(PDEVICE_OBJECT DeviceObject, PIRP Irp) {
    PAGED_CODE();
    UNREFERENCED_PARAMETER(DeviceObject);

    PIO_STACK_LOCATION irpSp = IoGetCurrentIrpStackLocation(Irp);
    ULONG ioControlCode = irpSp->Parameters.DeviceIoControl.IoControlCode;
    ULONG inputBufferLength = irpSp->Parameters.DeviceIoControl.InputBufferLength;
    ULONG outputBufferLength = irpSp->Parameters.DeviceIoControl.OutputBufferLength;
    PVOID pBuffer = Irp->AssociatedIrp.SystemBuffer;

    NTSTATUS ntStatus = STATUS_INVALID_DEVICE_REQUEST;
    ULONG_PTR bytesReturned = 0;

    switch (ioControlCode) {
    case IOCTL_OWL_GET_VERSION:
        if (outputBufferLength >= sizeof(ULONG)) {
            *((PULONG)pBuffer) = 0x00010000; // Version 1.0.0
            bytesReturned = sizeof(ULONG);
            ntStatus = STATUS_SUCCESS;
        } else {
            ntStatus = STATUS_BUFFER_TOO_SMALL;
        }
        break;

    case IOCTL_OWL_GET_DRIVER_STATUS:
        if (outputBufferLength >= sizeof(OWL_DRIVER_STATUS) && g_pSharedBridge) {
            g_pSharedBridge->GetStatus((POWL_DRIVER_STATUS)pBuffer);
            bytesReturned = sizeof(OWL_DRIVER_STATUS);
            ntStatus = STATUS_SUCCESS;
        } else {
            ntStatus = STATUS_BUFFER_TOO_SMALL;
        }
        break;

    case IOCTL_OWL_FEED_MIC_SAMPLES:
        if (inputBufferLength >= sizeof(OWL_AUDIO_CHUNK) && g_pSharedBridge) {
            POWL_AUDIO_CHUNK pChunk = (POWL_AUDIO_CHUNK)pBuffer;
            ntStatus = g_pSharedBridge->WriteMicSamples(
                pChunk->Samples,
                pChunk->FramesCount,
                pChunk->ChannelCount
            );
            bytesReturned = 0;
        } else {
            ntStatus = STATUS_INVALID_PARAMETER;
        }
        break;

    default:
        // Pass standard PortCls KsProperty IRPs to PortCls default handler
        return PcDispatchProperty(DeviceObject, Irp);
    }

    Irp->IoStatus.Status = ntStatus;
    Irp->IoStatus.Information = bytesReturned;
    IoCompleteRequest(Irp, IO_NO_INCREMENT);
    return ntStatus;
}
