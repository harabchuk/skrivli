cmake_minimum_required(VERSION 3.24)

project(MeetingBuddy LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

option(MEETINGBUDDY_BUILD_TESTS "Build unit tests" ON)

include(FetchContent)
# Official WTL 10.01 release (SourceForge). Do not track the GitHub mirror main branch.
FetchContent_Declare(
    wtl
    URL      https://downloads.sourceforge.net/project/wtl/WTL%2010/WTL%2010.01%20Release/WTL10_01_Release.zip
    URL_HASH SHA256=1A62EA728D088C7C5C7CFC76DB445E5C9F04923F92CADCF27B5D7678D85826A2
)
FetchContent_MakeAvailable(wtl)

# Glaze JSON (header-only). Requires C++23 and MSVC /Zc:preprocessor.
FetchContent_Declare(
    glaze
    GIT_REPOSITORY https://github.com/stephenberry/glaze.git
    GIT_TAG        v9.1.0
    GIT_SHALLOW    TRUE
)
FetchContent_MakeAvailable(glaze)

# whisper.cpp v1.9.5 — CPU only, static, portable AVX2 (no native host tuning).
set(WHISPER_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(WHISPER_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(WHISPER_BUILD_SERVER OFF CACHE BOOL "" FORCE)
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(GGML_NATIVE OFF CACHE BOOL "" FORCE)
set(GGML_AVX ON CACHE BOOL "" FORCE)
set(GGML_AVX2 ON CACHE BOOL "" FORCE)
set(GGML_FMA ON CACHE BOOL "" FORCE)
set(GGML_F16C ON CACHE BOOL "" FORCE)
set(GGML_CUDA OFF CACHE BOOL "" FORCE)
set(GGML_VULKAN OFF CACHE BOOL "" FORCE)
set(GGML_OPENMP OFF CACHE BOOL "" FORCE)
set(GGML_BLAS OFF CACHE BOOL "" FORCE)
FetchContent_Declare(
    whisper
    GIT_REPOSITORY https://github.com/ggml-org/whisper.cpp.git
    GIT_TAG        v1.9.5
    GIT_SHALLOW    TRUE
)
FetchContent_MakeAvailable(whisper)

# miniaudio 0.11.22 — WAV encode/decode, resampling, channel conversion, and mixing.
# Device I/O stays on WASAPI in this app. Keep these options in sync with src/audio/MiniAudio.hpp.
set(MINIAUDIO_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(MINIAUDIO_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_EXTRA_NODES ON CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_LIBVORBIS ON CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_LIBOPUS ON CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_DEVICEIO ON CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_THREADING ON CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_GENERATION ON CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_RESOURCE_MANAGER ON CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_NODE_GRAPH ON CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_ENGINE ON CACHE BOOL "" FORCE)
FetchContent_Declare(
    miniaudio
    GIT_REPOSITORY https://github.com/mackron/miniaudio.git
    GIT_TAG        0.11.22
    GIT_SHALLOW    TRUE
)
FetchContent_MakeAvailable(miniaudio)

set(MEETINGBUDDY_AUDIO_SOURCES
    src/audio/DeviceManager.cpp
    src/audio/CaptureStream.cpp
    src/audio/AudioResampler.cpp
    src/audio/MaConverter.cpp
    src/audio/SilentRenderKeepAlive.cpp
    src/audio/Mixer.cpp
    src/audio/WavHeader.cpp
    src/audio/WavWriter.cpp
    src/audio/M4aEncoder.cpp
    src/audio/RecordingSession.cpp
    src/audio/LevelMonitor.cpp
)

set(MEETINGBUDDY_TRANSCRIPTION_SOURCES
    src/transcription/AudioFileReader.cpp
    src/transcription/Resampler.cpp
    src/transcription/MarkdownFormatter.cpp
    src/transcription/WhisperEngine.cpp
    src/transcription/ModelManager.cpp
    src/transcription/TranscriptionService.cpp
)

set(MEETINGBUDDY_UTIL_SOURCES
    src/storage/Settings.cpp
    src/storage/RecordingSidecar.cpp
    src/storage/RecordingLibrary.cpp
    src/storage/DirectoryWatcher.cpp
    src/util/Paths.cpp
    src/util/Text.cpp
    src/util/FileIo.cpp
    src/util/Log.cpp
    src/util/HResult.cpp
    src/util/Autostart.cpp
    src/util/Hotkey.cpp
)

add_executable(MeetingBuddy WIN32
    src/main.cpp
    src/Application.cpp
    src/ui/MainWindow.cpp
    src/ui/DeviceCombo.cpp
    src/ui/RecordView.cpp
    src/ui/RecordingsView.cpp
    src/ui/ShellDialogs.cpp
    src/ui/SettingsDialog.cpp
    src/ui/TabCtrl.cpp
    src/ui/TranscriptWindow.cpp
    src/ui/TrayIcon.cpp
    src/ui/LevelMeter.cpp
    src/ui/RecordButton.cpp
    ${MEETINGBUDDY_AUDIO_SOURCES}
    ${MEETINGBUDDY_TRANSCRIPTION_SOURCES}
    ${MEETINGBUDDY_UTIL_SOURCES}
    src/util/Clipboard.cpp
    resources/MeetingBuddy.rc
)

target_include_directories(MeetingBuddy PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/src
    ${CMAKE_CURRENT_SOURCE_DIR}/resources
    ${wtl_SOURCE_DIR}/Include
)

target_compile_definitions(MeetingBuddy PRIVATE
    UNICODE
    _UNICODE
    WIN32_LEAN_AND_MEAN
    _WTL_USE_CSTRING
    NOMINMAX
)

target_link_libraries(MeetingBuddy PRIVATE
    whisper
    miniaudio
    glaze::glaze
    ole32
    uuid
    shell32
    shlwapi
    comctl32
    advapi32
    avrt
    propsys
    winmm
    mfplat
    mf
    mfreadwrite
    mfuuid
    wmcodecdspuuid
    winhttp
    bcrypt
    crypt32
    uxtheme
)

if(MSVC)
    target_compile_options(MeetingBuddy PRIVATE /W4 /permissive- /utf-8 /Zc:preprocessor)
    # Use main() with a WIN32 subsystem executable.
    # Manifest is embedded from resources/MeetingBuddy.rc — disable the linker default.
    target_link_options(MeetingBuddy PRIVATE /ENTRY:mainCRTStartup /MANIFEST:NO)
endif()

if(MEETINGBUDDY_BUILD_TESTS)
    set(gtest_force_shared_crt ON CACHE BOOL "" FORCE)
    FetchContent_Declare(
        googletest
        GIT_REPOSITORY https://github.com/google/googletest.git
        GIT_TAG        v1.15.2
    )
    FetchContent_MakeAvailable(googletest)

    enable_testing()

    add_executable(MeetingBuddyTests
        tests/RingBufferTests.cpp
        tests/WavWriterTests.cpp
        tests/MixerTests.cpp
        tests/RecordingLibraryTests.cpp
        tests/ResamplerTests.cpp
        tests/MarkdownFormatterTests.cpp
        tests/ChunkPlanTests.cpp
        tests/SettingsTests.cpp
        tests/TextTests.cpp
        tests/FileIoTests.cpp
        tests/WavHeaderTests.cpp
        src/storage/Settings.cpp
        src/audio/Mixer.cpp
        src/audio/MaConverter.cpp
        src/audio/WavHeader.cpp
        src/audio/WavWriter.cpp
        src/storage/RecordingSidecar.cpp
        src/storage/RecordingLibrary.cpp
        src/transcription/Resampler.cpp
        src/transcription/MarkdownFormatter.cpp
        src/util/Paths.cpp
        src/util/Text.cpp
        src/util/FileIo.cpp
        src/util/Log.cpp
        src/util/HResult.cpp
    )

    target_include_directories(MeetingBuddyTests PRIVATE
        ${CMAKE_CURRENT_SOURCE_DIR}/src
    )

    target_compile_definitions(MeetingBuddyTests PRIVATE
        UNICODE
        _UNICODE
        WIN32_LEAN_AND_MEAN
        NOMINMAX
    )

    target_link_libraries(MeetingBuddyTests PRIVATE
        GTest::gtest_main
        miniaudio
        glaze::glaze
        ole32
        uuid
        shell32
        shlwapi
        advapi32
    )

    if(MSVC)
        target_compile_options(MeetingBuddyTests PRIVATE /W4 /permissive- /utf-8 /Zc:preprocessor)
    endif()

    include(GoogleTest)
    gtest_discover_tests(MeetingBuddyTests DISCOVERY_MODE PRE_TEST)
endif()
