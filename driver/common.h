#pragma once

#ifndef _COMMON_H_
#define _COMMON_H_

#include <ntddk.h>
#include <windef.h>
#include <ks.h>
#include <ksmedia.h>
#include <portcls.h>

#define OWL_POOL_TAG 'LwOl' // 'lOwL' in little endian

// Sample rates & format constants
#define OWL_DEFAULT_SAMPLE_RATE    48000
#define OWL_DEFAULT_CHANNELS       2
#define OWL_DEFAULT_BITS_PER_SAMPLE 32
#define OWL_MAX_BUFFER_SIZE        (128 * 1024) // 128 KB DMA ring buffer
#define OWL_MIN_BUFFER_SIZE        (8 * 1024)   // 8 KB minimum buffer

// IOCTL Definitions for OpenWaveLink IPC Bridge
#define FILE_DEVICE_OPENWAVELINK 0x8240

#define IOCTL_OWL_GET_VERSION \
    CTL_CODE(FILE_DEVICE_OPENWAVELINK, 0x801, METHOD_BUFFERED, FILE_ANY_ACCESS)

#define IOCTL_OWL_FEED_MIC_SAMPLES \
    CTL_CODE(FILE_DEVICE_OPENWAVELINK, 0x802, METHOD_BUFFERED, FILE_WRITE_ACCESS)

#define IOCTL_OWL_GET_DRIVER_STATUS \
    CTL_CODE(FILE_DEVICE_OPENWAVELINK, 0x803, METHOD_BUFFERED, FILE_READ_ACCESS)

#define IOCTL_OWL_MAP_STREAM_PIPE \
    CTL_CODE(FILE_DEVICE_OPENWAVELINK, 0x804, METHOD_BUFFERED, FILE_ANY_ACCESS)

// Driver Status payload
#pragma pack(push, 1)
typedef struct _OWL_DRIVER_STATUS {
    ULONG  DriverVersionMajor;
    ULONG  DriverVersionMinor;
    ULONG  ActiveStreamsCount;
    ULONG  RenderActive;
    ULONG  MicFXActive;
    ULONG  StreamInActive;
    UINT64 TotalFramesTransferred;
    ULONG  SampleRate;
    ULONG  BufferSizeBytes;
} OWL_DRIVER_STATUS, *POWL_DRIVER_STATUS;

// Sample chunk passed from User-Mode owl-core
typedef struct _OWL_AUDIO_CHUNK {
    ULONG  ChannelCount;
    ULONG  SampleRate;
    ULONG  FramesCount;
    FLOAT  Samples[1024 * 2]; // Interleaved stereo 32-bit float samples
} OWL_AUDIO_CHUNK, *POWL_AUDIO_CHUNK;
#pragma pack(pop)

// Forward declarations
class CMiniportWaveRT;
class CMiniportWaveRTStream;
class CMiniportTopology;

#endif // _COMMON_H_
