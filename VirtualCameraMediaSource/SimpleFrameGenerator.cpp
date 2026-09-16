//
// Copyright (C) Microsoft Corporation. All rights reserved.
//
#include "pch.h"

namespace
{
    constexpr wchar_t PipeName[] = L"\\\\.\\pipe\\CamBridge.Video.v1";
    constexpr wchar_t PipeNameV2[] = L"\\\\.\\pipe\\CamBridge.Video.v2";
    constexpr DWORD MaxFrameBytes = 128 * 1024 * 1024;

    bool ReadExact(HANDLE pipe, BYTE* data, DWORD length)
    {
        DWORD offset = 0;
        while (offset < length)
        {
            DWORD received = 0;
            if (!ReadFile(pipe, data + offset, length - offset, &received, nullptr) || received == 0)
                return false;
            offset += received;
        }
        return true;
    }
}

SimpleFrameGenerator::~SimpleFrameGenerator()
{
    m_running = false;
    if (m_worker.joinable())
    {
        CancelSynchronousIo(m_worker.native_handle());
        m_worker.join();
    }
}

void SimpleFrameGenerator::ReceiveFrames()
{
    while (m_running)
    {
        const bool v2 = WaitNamedPipeW(PipeNameV2, 100) != FALSE;
        if (!v2 && !WaitNamedPipeW(PipeName, 250)) continue;
        HANDLE pipe = CreateFileW(v2 ? PipeNameV2 : PipeName, GENERIC_READ, 0, nullptr, OPEN_EXISTING, 0, nullptr);
        if (pipe == INVALID_HANDLE_VALUE) continue;
        while (m_running)
        {
            DWORD length = 0;
            DWORD width = 1920, height = 1080;
            if (v2 && (!ReadExact(pipe, reinterpret_cast<BYTE*>(&width), 4) || !ReadExact(pipe, reinterpret_cast<BYTE*>(&height), 4))) break;
            if (!ReadExact(pipe, reinterpret_cast<BYTE*>(&length), sizeof(length))) break;
            if (width == 0 || height == 0 || width > 8192 || height > 8192 || length > MaxFrameBytes ||
                static_cast<ULONGLONG>(width) * height * 4 != length) break;
            std::vector<BYTE> next(length);
            if (!ReadExact(pipe, next.data(), length)) break;
            std::lock_guard<std::mutex> lock(m_frameLock);
            m_latestFrame.swap(next);
            m_inputWidth = width;
            m_inputHeight = height;
            m_latestFrameAt = GetTickCount64();
        }
        CloseHandle(pipe);
    }
}

HRESULT SimpleFrameGenerator::Initialize(_In_ IMFMediaType* pMediaType)
{
    RETURN_HR_IF_NULL(E_INVALIDARG, pMediaType);

    RETURN_IF_FAILED(pMediaType->GetGUID(MF_MT_SUBTYPE, &m_subType));
    if (m_subType != MFVideoFormat_RGB32 && m_subType != MFVideoFormat_NV12)
    {
        RETURN_HR_MSG(MF_E_UNSUPPORTED_FORMAT, "Unsupported format: %s", winrt::to_hstring(m_subType).data());
    }
    MFGetAttributeSize(pMediaType, MF_MT_FRAME_SIZE, &m_width, &m_height);

    if (!m_running.exchange(true))
        m_worker = std::thread(&SimpleFrameGenerator::ReceiveFrames, this);

    return S_OK;
}

/*:
   Writes to a buffer representing a 2D image.
   Writes a different constant to each line based on row number and current time.
   Assumes top down image, no negative stride and pBuf points to the begnning of the buffer of length len.
   Param:
   pBuf - pointer to beginning of buffer
   pitch - line length in bytes
   len - length of buffer in bytes
*/
HRESULT SimpleFrameGenerator::CreateFrame(
    _Inout_updates_bytes_(len) BYTE* pBuf,
    _In_ DWORD len,
    _In_ LONG pitch,
    _In_ ULONG rgbMask)
{
    std::vector<BYTE> frame;
    UINT32 inputWidth = 0, inputHeight = 0;
    {
        std::lock_guard<std::mutex> lock(m_frameLock);
        if (GetTickCount64() - m_latestFrameAt < 2000) {
            frame = m_latestFrame;
            inputWidth = m_inputWidth;
            inputHeight = m_inputHeight;
        }
    }
    if (frame.empty() || frame.size() != static_cast<size_t>(inputWidth) * inputHeight * 4)
    {
        ZeroMemory(pBuf, len);
        return S_OK;
    }
    if (inputWidth != m_width || inputHeight != m_height) {
        std::vector<BYTE> resized(static_cast<size_t>(m_width) * m_height * 4);
        for (UINT32 y = 0; y < m_height; ++y) {
            const UINT32 sy = static_cast<UINT32>(static_cast<UINT64>(y) * inputHeight / m_height);
            for (UINT32 x = 0; x < m_width; ++x) {
                const UINT32 sx = static_cast<UINT32>(static_cast<UINT64>(x) * inputWidth / m_width);
                memcpy(resized.data() + (static_cast<size_t>(y) * m_width + x) * 4,
                    frame.data() + (static_cast<size_t>(sy) * inputWidth + sx) * 4, 4);
            }
        }
        frame.swap(resized);
    }

    if (m_subType == MFVideoFormat_RGB32)
    {
        RETURN_HR_IF(E_INVALIDARG, len < static_cast<DWORD>(abs(pitch)) * m_height);
        for (DWORD row = 0; row < m_height; ++row)
            memcpy(pBuf + row * pitch, frame.data() + row * m_width * 4, m_width * 4);
    }
    else if(m_subType == MFVideoFormat_NV12)
    {
        DEBUG_MSG(L"NV12 frames %s \n", winrt::to_hstring(MFVideoFormat_NV12).data());

        RETURN_IF_FAILED(RGB32ToNV12Frame(frame.data(), static_cast<ULONG>(frame.size()), m_width * 4, m_width, m_height, pBuf, len, pitch));
    }
    else
    {
        return MF_E_UNSUPPORTED_FORMAT;
    }

    return S_OK;
}

//////////////////////////////////////////////////
// private

HRESULT SimpleFrameGenerator::_CreateRGB32Frame(
    _Inout_updates_bytes_(len) BYTE* pBuf,
    _In_ DWORD len,
    _In_ LONG pitch,
    _In_ DWORD width,
    _In_ DWORD height,
    _In_ ULONG rgbMask )
{
    RETURN_HR_IF_NULL(E_INVALIDARG, pBuf);
    if (len < (abs(pitch) * height ))
    {
        return HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER);
    }

    LONGLONG curSysTimeInS = MFGetSystemTime() / (MFTIME)10000000;
    int offset = curSysTimeInS % height;

    for (unsigned int r = 0; r < height; r++)
    {
        uint32_t* p = (uint32_t*)(pBuf + (r * pitch));
        for (unsigned int c = 0; c < width; c++)
        {
            BYTE gray = (BYTE)(r + offset);
            *p = ((uint32_t)gray << 16 | (uint32_t)gray << 8 | (uint32_t)gray) & rgbMask;
            p++;
        }
    }

    return S_OK;
}

//////////////////////////////////////////////////
// pixelFormatConverter

void SimpleFrameGenerator::RGB24ToYUY2(int R, int G, int B, BYTE* pY, BYTE* pU, BYTE* pV)
{
    *pY = ((66 * R + 129 * G + 25 * B + 128) >> 8) + 16;
    *pU = ((-38 * R - 74 * G + 112 * B + 128) >> 8) + 128;
    *pV = ((112 * R - 94 * G - 18 * B + 128) >> 8) + 128;
}

void SimpleFrameGenerator::RGB24ToY(int R, int G, int B, BYTE* pY)
{
    *pY = ((66 * R + 129 * G + 25 * B + 128) >> 8) + 16;
}

void SimpleFrameGenerator::RGB32ToNV12(BYTE RGB1[8], BYTE RGB2[8], BYTE* pY1, BYTE* pY2, BYTE* pUV)
{
    RGB24ToYUY2(RGB1[2], RGB1[1], RGB1[0], pY1, pUV, pUV + 1);
    RGB24ToY(RGB1[6], RGB1[5], RGB1[4], pY1 + 1);
    RGB24ToYUY2(RGB2[2], RGB2[1], RGB2[0], pY2, pUV, pUV + 1);
    RGB24ToY(RGB2[6], RGB2[5], RGB2[4], pY2 + 1);
};

//////////////////////////////////////////////////
// FrameFormatConverter

HRESULT SimpleFrameGenerator::RGB32ToNV12Frame(_Inout_updates_bytes_(len) BYTE* pbBuff, ULONG cbBuff, long stride, UINT width, UINT height, BYTE* pbBuffOut, ULONG cbBuffOut, long strideOut)
{
    do
    {
        RETURN_HR_IF(E_UNEXPECTED, width * 4 * height > cbBuff);
        RETURN_HR_IF(E_UNEXPECTED, width * 1.5 * height > cbBuffOut);
        RETURN_HR_IF_NULL(E_INVALIDARG, pbBuff);

        RETURN_HR_IF_NULL(E_INVALIDARG, pbBuffOut);
        for (DWORD h = 0; h < height - 1; h += 2)
        {
            BYTE* pRGB1 = h * stride + pbBuff;
            BYTE* pRGB2 = (h + 1) * stride + pbBuff;
            BYTE* pY1 = h * strideOut + pbBuffOut;
            BYTE* pY2 = (h + 1) * strideOut + pbBuffOut;
            BYTE* pUV = (h / 2 + height) * strideOut + pbBuffOut;

            for (DWORD w = 0; w < width; w += 2)
            {
                RGB32ToNV12(pRGB1, pRGB2, pY1, pY2, pUV);
                pRGB1 += 8;
                pRGB2 += 8;
                pY1 += 2;
                pY2 += 2;
                pUV += 2;
            }
        }
    } while (FALSE);

    return S_OK;
}
