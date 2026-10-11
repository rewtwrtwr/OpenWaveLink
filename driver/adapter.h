#pragma once

#ifndef _ADAPTER_H_
#define _ADAPTER_H_

#include "common.h"
#include "minwavert.h"
#include "mintopo.h"
#include "shared_bridge.h"

#define OWL_DEVICE_NAME     L"\\Device\\OpenWaveLinkAudio"
#define OWL_DOS_DEVICE_NAME L"\\DosDevices\\OpenWaveLinkAudio"

// PnP and IRP Dispatch Prototypes
DRIVER_INITIALIZE DriverEntry;
DRIVER_ADD_DEVICE AddDevice;
DRIVER_UNLOAD     DriverUnload;

NTSTATUS OwlDispatchCreate(PDEVICE_OBJECT DeviceObject, PIRP Irp);
NTSTATUS OwlDispatchClose(PDEVICE_OBJECT DeviceObject, PIRP Irp);
NTSTATUS OwlDispatchDeviceControl(PDEVICE_OBJECT DeviceObject, PIRP Irp);

#endif // _ADAPTER_H_
