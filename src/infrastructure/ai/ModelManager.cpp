#include "infrastructure/ai/ModelManager.hpp"

#include "infrastructure/common/FileIo.hpp"
#include "infrastructure/common/Log.hpp"
#include "infrastructure/common/Paths.hpp"
#include "infrastructure/common/Text.hpp"

#include <algorithm>
#include <bcrypt.h>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <string>
#include <vector>
#include <wil/resource.h>
#include <winhttp.h>

#pragma comment(lib, "bcrypt.lib")
#pragma comment(lib, "winhttp.lib")

namespace {

using UniqueWinHttp =
    wil::unique_any<HINTERNET, decltype(&::WinHttpCloseHandle), ::WinHttpCloseHandle>;
using ProgressFn = ModelManager::ProgressFn;

constexpr wchar_t kHost[] = L"huggingface.co";
constexpr wchar_t kPathPrefix[] = L"/ggerganov/whisper.cpp/resolve/main/";

auto ToLowerHex(const BYTE* data, DWORD len) -> std::string {
    static const char* kDigits = "0123456789abcdef";
    std::string out;
    out.resize(static_cast<size_t>(len) * 2);
    for (DWORD i = 0; i < len; ++i) {
        out[i * 2] = kDigits[(data[i] >> 4) & 0xF];
        out[i * 2 + 1] = kDigits[data[i] & 0xF];
    }
    return out;
}

struct DownloadState {
    std::atomic_bool* cancel = nullptr;
    const ProgressFn* onProgress = nullptr;
    uint64_t total = 0;
    uint64_t done = 0;
};

void Report(DownloadState* state) {
    if (state != nullptr && state->onProgress != nullptr && *state->onProgress) {
        (*state->onProgress)(state->done, state->total);
    }
}

struct HttpResponse {
    // Declared in open order so they close in reverse (request, connect, session).
    UniqueWinHttp session;
    UniqueWinHttp connect;
    UniqueWinHttp request;
    DWORD status = 0;
    uint64_t contentLength = 0;
};

bool OpenRequest(const std::wstring& fileName, uint64_t resumeFrom, HttpResponse& response,
                 std::wstring& error) {
    response.session.reset(::WinHttpOpen(L"MeetingBuddy/1.0", WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
                                         WINHTTP_NO_PROXY_NAME, WINHTTP_NO_PROXY_BYPASS, 0));
    if (!response.session) {
        error = L"WinHttpOpen failed.";
        return false;
    }

    response.connect.reset(
        ::WinHttpConnect(response.session.get(), kHost, INTERNET_DEFAULT_HTTPS_PORT, 0));
    if (!response.connect) {
        error = L"WinHttpConnect failed.";
        return false;
    }

    const auto object = std::wstring(kPathPrefix) + fileName;
    response.request.reset(::WinHttpOpenRequest(response.connect.get(), L"GET", object.c_str(),
                                                nullptr, WINHTTP_NO_REFERER,
                                                WINHTTP_DEFAULT_ACCEPT_TYPES, WINHTTP_FLAG_SECURE));
    if (!response.request) {
        error = L"WinHttpOpenRequest failed.";
        return false;
    }

    std::wstring headers;
    if (resumeFrom > 0) {
        wchar_t range[128]{};
        std::swprintf(range, 128, L"Range: bytes=%llu-\r\n",
                      static_cast<unsigned long long>(resumeFrom));
        headers = range;
    }

    const BOOL sent = ::WinHttpSendRequest(
        response.request.get(), headers.empty() ? WINHTTP_NO_ADDITIONAL_HEADERS : headers.c_str(),
        headers.empty() ? 0 : static_cast<DWORD>(-1), WINHTTP_NO_REQUEST_DATA, 0, 0, 0);
    if (!sent || !::WinHttpReceiveResponse(response.request.get(), nullptr)) {
        error = L"HTTP request failed.";
        return false;
    }

    DWORD statusSize = sizeof(response.status);
    ::WinHttpQueryHeaders(
        response.request.get(), WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
        WINHTTP_HEADER_NAME_BY_INDEX, &response.status, &statusSize, WINHTTP_NO_HEADER_INDEX);

    if (response.status != 200 && response.status != 206) {
        error = L"Model download HTTP status " + std::to_wstring(response.status);
        return false;
    }

    wchar_t lengthBuf[64]{};
    DWORD lengthSize = sizeof(lengthBuf);
    if (::WinHttpQueryHeaders(response.request.get(), WINHTTP_QUERY_CONTENT_LENGTH,
                              WINHTTP_HEADER_NAME_BY_INDEX, lengthBuf, &lengthSize,
                              WINHTTP_NO_HEADER_INDEX)) {
        response.contentLength = _wcstoui64(lengthBuf, nullptr, 10);
    }
    return true;
}

bool StreamToFile(const HttpResponse& response, const std::wstring& partial, uint64_t existing,
                  DownloadState* progress, std::wstring& error) {
    const bool resumed = existing > 0 && response.status == 206;

    if (progress != nullptr) {
        progress->done = existing;
        progress->total = existing + response.contentLength;
        if (progress->total == 0) {
            progress->total = 1;
        }
    }

    const auto rawFile =
        ::CreateFileW(partial.c_str(), GENERIC_WRITE, FILE_SHARE_READ, nullptr,
                      resumed ? OPEN_ALWAYS : CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (rawFile == INVALID_HANDLE_VALUE) {
        error = L"Could not create download file.";
        return false;
    }
    wil::unique_handle file(rawFile);

    if (resumed) {
        LARGE_INTEGER move{};
        move.QuadPart = static_cast<LONGLONG>(existing);
        ::SetFilePointerEx(file.get(), move, nullptr, FILE_BEGIN);
    } else if (progress != nullptr) {
        progress->done = 0;
        progress->total = response.contentLength > 0 ? response.contentLength : 1;
    }

    std::vector<BYTE> buffer(64 * 1024);
    for (;;) {
        if (progress != nullptr && progress->cancel != nullptr && progress->cancel->load()) {
            error = L"Download cancelled.";
            return false;
        }

        DWORD available = 0;
        if (!::WinHttpQueryDataAvailable(response.request.get(), &available)) {
            error = L"WinHttpQueryDataAvailable failed.";
            return false;
        }
        if (available == 0) {
            return true;
        }

        const auto toRead = (std::min)(available, static_cast<DWORD>(buffer.size()));
        DWORD read = 0;
        if (!::WinHttpReadData(response.request.get(), buffer.data(), toRead, &read) || read == 0) {
            error = L"WinHttpReadData failed.";
            return false;
        }

        DWORD written = 0;
        if (!::WriteFile(file.get(), buffer.data(), read, &written, nullptr) || written != read) {
            error = L"Failed writing download.";
            return false;
        }

        if (progress != nullptr) {
            progress->done += read;
            Report(progress);
        }
    }
}

bool HttpDownloadToFile(const std::wstring& fileName, const std::wstring& destPath,
                        DownloadState* progress, std::wstring& error) {
    const auto partial = destPath + L".partial";
    const auto existing = FileIo::FileSize(partial);

    {
        HttpResponse response;
        if (!OpenRequest(fileName, existing, response, error) ||
            !StreamToFile(response, partial, existing, progress, error)) {
            return false;
        }
    }

    std::error_code ec;
    std::filesystem::rename(partial, destPath, ec);
    if (ec) {
        std::filesystem::remove(destPath, ec);
        std::filesystem::rename(partial, destPath, ec);
        if (ec) {
            error = L"Could not finalize downloaded model.";
            return false;
        }
    }
    return true;
}

} // namespace

auto ModelManager::OfferedModels() -> const std::vector<ModelInfo>& {
    // SHA-256 digests from Hugging Face LFS (ggerganov/whisper.cpp).
    static const std::vector<ModelInfo> kModels = {
        {L"ggml-tiny.bin", L"tiny",
         "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21"},
        {L"ggml-tiny.en.bin", L"tiny.en",
         "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f"},
        {L"ggml-tiny-q5_1.bin", L"tiny-q5_1",
         "818710568da3ca15689e31a743197b520007872ff9576237bda97bd1b469c3d7"},
        {L"ggml-tiny.en-q5_1.bin", L"tiny.en-q5_1",
         "c77c5766f1cef09b6b7d47f21b546cbddd4157886b3b5d6d4f709e91e66c7c2b"},
        {L"ggml-base.bin", L"base",
         "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe"},
        {L"ggml-base.en.bin", L"base.en",
         "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002"},
        {L"ggml-base-q5_1.bin", L"base-q5_1 (default)",
         "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898"},
        {L"ggml-base.en-q5_1.bin", L"base.en-q5_1",
         "4baf70dd0d7c4247ba2b81fafd9c01005ac77c2f9ef064e00dcf195d0e2fdd2f"},
        {L"ggml-small.bin", L"small",
         "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b"},
        {L"ggml-small.en.bin", L"small.en",
         "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d"},
        {L"ggml-small-q5_1.bin", L"small-q5_1",
         "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb"},
        {L"ggml-small.en-q5_1.bin", L"small.en-q5_1",
         "bfdff4894dcb76bbf647d56263ea2a96645423f1669176f4844a1bf8e478ad30"},
    };
    return kModels;
}

auto ModelManager::Find(const std::wstring& fileName) -> const ModelInfo* {
    for (const auto& model : OfferedModels()) {
        if (_wcsicmp(model.fileName.c_str(), fileName.c_str()) == 0) {
            return &model;
        }
    }
    return nullptr;
}

auto ModelManager::ModelPath(const std::wstring& fileName) -> std::wstring {
    return (std::filesystem::path(Paths::ModelsDir()) / fileName).wstring();
}

bool ModelManager::IsPresent(const std::wstring& fileName) {
    return std::filesystem::exists(ModelPath(fileName));
}

auto ModelManager::VadModelPath() -> std::wstring {
    return (std::filesystem::path(Paths::ModelsDir()) / L"ggml-silero-v5.1.2.bin").wstring();
}

bool ModelManager::VadModelPresent() {
    return std::filesystem::exists(VadModelPath());
}

auto ModelManager::Sha256Hex(const std::wstring& path) -> std::wstring {
    std::ifstream file(path, std::ios::binary);
    if (!file) {
        return {};
    }

    BCRYPT_ALG_HANDLE alg = nullptr;
    BCRYPT_HASH_HANDLE hash = nullptr;
    DWORD objectLen = 0;
    DWORD dataLen = 0;
    DWORD hashLen = 0;

    if (FAILED(::BCryptOpenAlgorithmProvider(&alg, BCRYPT_SHA256_ALGORITHM, nullptr, 0))) {
        return {};
    }
    ::BCryptGetProperty(alg, BCRYPT_OBJECT_LENGTH, reinterpret_cast<PUCHAR>(&objectLen),
                        sizeof(objectLen), &dataLen, 0);
    ::BCryptGetProperty(alg, BCRYPT_HASH_LENGTH, reinterpret_cast<PUCHAR>(&hashLen),
                        sizeof(hashLen), &dataLen, 0);

    std::vector<BYTE> object(objectLen);
    std::vector<BYTE> digest(hashLen);
    if (FAILED(::BCryptCreateHash(alg, &hash, object.data(), objectLen, nullptr, 0, 0))) {
        ::BCryptCloseAlgorithmProvider(alg, 0);
        return {};
    }

    std::vector<char> buffer(1024 * 1024);
    while (file) {
        file.read(buffer.data(), static_cast<std::streamsize>(buffer.size()));
        const auto got = file.gcount();
        if (got <= 0) {
            break;
        }
        ::BCryptHashData(hash, reinterpret_cast<PUCHAR>(buffer.data()), static_cast<ULONG>(got), 0);
    }

    ::BCryptFinishHash(hash, digest.data(), hashLen, 0);
    ::BCryptDestroyHash(hash);
    ::BCryptCloseAlgorithmProvider(alg, 0);
    return Text::Utf8ToWide(ToLowerHex(digest.data(), hashLen));
}

bool ModelManager::VerifySha256(const std::wstring& path, const char* expectedHex) {
    if (expectedHex == nullptr) {
        return false;
    }
    const auto actual = Sha256Hex(path);
    const auto expected = Text::Utf8ToWide(expectedHex);
    return _wcsicmp(actual.c_str(), expected.c_str()) == 0;
}

bool ModelManager::DownloadToFile(const ModelInfo& info, std::atomic_bool& cancel,
                                  const ProgressFn& onProgress, std::wstring& error) {
    Paths::EnsureDirectory(Paths::ModelsDir());
    const auto dest = ModelPath(info.fileName);

    DownloadState progress{};
    progress.cancel = &cancel;
    progress.onProgress = &onProgress;
    const bool ok = HttpDownloadToFile(info.fileName, dest, &progress, error);
    if (!ok) {
        std::error_code ec;
        std::filesystem::remove(dest + L".partial", ec);
        std::filesystem::remove(dest, ec);
        return false;
    }

    if (!VerifySha256(dest, info.sha256)) {
        error = L"SHA-256 mismatch; download deleted.";
        std::error_code ec;
        std::filesystem::remove(dest, ec);
        Log::Error(L"SHA-256 mismatch for " + info.fileName);
        return false;
    }
    return true;
}
