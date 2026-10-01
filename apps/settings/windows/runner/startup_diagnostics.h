#ifndef RUNNER_STARTUP_DIAGNOSTICS_H_
#define RUNNER_STARTUP_DIAGNOSTICS_H_

#include <windows.h>
#include <cstdio>
#include <cstring>
#include <string>

namespace settings_startup {
inline const ULONGLONG process_started = GetTickCount64();
inline DWORD opened_at = GetTickCount();

// Small, bounded diagnostic log; never records typed text or settings contents.
inline void Log(const char* event) {
  SYSTEMTIME now{};
  GetSystemTime(&now);
  char line[384]{};
  snprintf(line, sizeof(line),
           "%04u-%02u-%02uT%02u:%02u:%02u.%03uZ pid=%lu event=%s "
           "process_ms=%llu open_ms=%lu\r\n",
           now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute,
           now.wSecond, now.wMilliseconds, GetCurrentProcessId(), event,
           GetTickCount64() - process_started, GetTickCount() - opened_at);
  OutputDebugStringA(line);
  wchar_t local[32768]{};
  const DWORD length = GetEnvironmentVariableW(L"LOCALAPPDATA", local, 32768);
  if (!length || length >= 32768) return;
  std::wstring directory = std::wstring(local) + L"\\retype";
  CreateDirectoryW(directory.c_str(), nullptr);
  directory += L"\\logs";
  CreateDirectoryW(directory.c_str(), nullptr);
  const auto path = directory + L"\\settings-startup.log";
  WIN32_FILE_ATTRIBUTE_DATA info{};
  if (GetFileAttributesExW(path.c_str(), GetFileExInfoStandard, &info) &&
      (info.nFileSizeHigh || info.nFileSizeLow > 256 * 1024)) {
    MoveFileExW(path.c_str(), (path + L".previous").c_str(),
                MOVEFILE_REPLACE_EXISTING);
  }
  HANDLE file = CreateFileW(path.c_str(), FILE_APPEND_DATA,
      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
      OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
  if (file == INVALID_HANDLE_VALUE) return;
  DWORD written = 0;
  WriteFile(file, line, static_cast<DWORD>(strlen(line)), &written, nullptr);
  CloseHandle(file);
}

inline std::wstring ExecutablePath() {
  wchar_t path[32768]{};
  DWORD length = GetModuleFileNameW(nullptr, path, 32768);
  return length && length < 32768 ? std::wstring(path, length) : std::wstring();
}

inline HWND FindSameVersionWindow(const std::wstring& path) {
  HWND window = nullptr;
  while ((window = FindWindowExW(nullptr, window,
      L"FLUTTER_RUNNER_WIN32_WINDOW", L"retype 设置"))) {
    DWORD pid = 0;
    GetWindowThreadProcessId(window, &pid);
    HANDLE process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
    if (!process) continue;
    wchar_t found[32768]{};
    DWORD length = 32768;
    bool matches = QueryFullProcessImageNameW(process, 0, found, &length) &&
        CompareStringOrdinal(path.c_str(), -1, found, -1, TRUE) == CSTR_EQUAL;
    CloseHandle(process);
    if (matches) return window;
    Log("reuse.version_mismatch");
  }
  return nullptr;
}

inline std::wstring InstanceName(std::wstring path) {
  CharLowerBuffW(path.data(), static_cast<DWORD>(path.size()));
  unsigned long long hash = 14695981039346656037ULL;
  for (wchar_t c : path) {
    hash ^= static_cast<unsigned short>(c);
    hash *= 1099511628211ULL;
  }
  return L"Local\\retype-settings-" + std::to_wstring(hash);
}
}  // namespace settings_startup
#endif
