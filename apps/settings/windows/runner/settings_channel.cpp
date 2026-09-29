#include "settings_channel.h"
#include "flutter_window.h"

#include <flutter/flutter_engine.h>
#include <flutter/standard_method_codec.h>
#include <shellapi.h>
#include <windows.h>

#include <cstdint>
#include <string>

namespace {
constexpr wchar_t kPreferences[] = L"Software\\retype";

bool ReadDword(const wchar_t* name, DWORD* value) {
  DWORD bytes = sizeof(*value);
  return RegGetValueW(HKEY_CURRENT_USER, kPreferences, name, RRF_RT_REG_DWORD,
                      nullptr, value, &bytes) == ERROR_SUCCESS;
}

bool WriteDword(const wchar_t* name, DWORD value) {
  HKEY key = nullptr;
  if (RegCreateKeyExW(HKEY_CURRENT_USER, kPreferences, 0, nullptr, 0,
                      KEY_SET_VALUE, nullptr, &key, nullptr) != ERROR_SUCCESS) {
    return false;
  }
  const auto result = RegSetValueExW(key, name, 0, REG_DWORD,
                                     reinterpret_cast<const BYTE*>(&value),
                                     sizeof(value));
  RegCloseKey(key);
  return result == ERROR_SUCCESS;
}

std::wstring ReadInstalledString(const wchar_t* name) {
  wchar_t buffer[32768] = {};
  DWORD bytes = sizeof(buffer);
  const auto result = RegGetValueW(HKEY_LOCAL_MACHINE, kPreferences, name,
                                   RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
                                   nullptr, buffer, &bytes);
  return result == ERROR_SUCCESS ? std::wstring(buffer) : std::wstring();
}

std::string Utf8(const std::wstring& value) {
  const int count = WideCharToMultiByte(CP_UTF8, 0, value.c_str(), -1, nullptr,
                                        0, nullptr, nullptr);
  if (count <= 0) return {};
  std::string result(count, '\0');
  WideCharToMultiByte(CP_UTF8, 0, value.c_str(), -1, result.data(), count,
                      nullptr, nullptr);
  result.resize(count - 1);
  return result;
}

std::wstring Utf16(const std::string& value) {
  const int count = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS,
                                        value.data(), static_cast<int>(value.size()),
                                        nullptr, 0);
  if (count <= 0) return {};
  std::wstring result(count, L'\0');
  MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(),
                      static_cast<int>(value.size()), result.data(), count);
  return result;
}

HANDLE installer_process = nullptr;

bool OpenPath(const std::wstring& path) {
  return reinterpret_cast<INT_PTR>(
             ShellExecuteW(nullptr, L"open", path.c_str(), nullptr, nullptr,
                           SW_SHOWNORMAL)) > 32;
}

}  // namespace

std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>>
RegisterSettingsChannel(flutter::FlutterEngine* engine, HWND window) {
  auto channel = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      engine->messenger(), "retype/settings",
      &flutter::StandardMethodCodec::GetInstance());
  channel->SetMethodCallHandler(
      [window](const flutter::MethodCall<flutter::EncodableValue>& call,
         std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result) {
        const auto& method = call.method_name();
        if (method == "closeWindow") {
          PostMessageW(window, kHideSettingsMessage, 0, 0);
          result->Success();
          return;
        }
        if (method == "startDrag") {
          POINT cursor{};
          GetCursorPos(&cursor);
          ReleaseCapture();
          PostMessageW(window, WM_NCLBUTTONDOWN, HTCAPTION,
                       MAKELPARAM(cursor.x, cursor.y));
          result->Success();
          return;
        }
        if (method == "getSettings") {
          DWORD scheme = 0;
          DWORD auto_check = 0;
          const bool has_auto_check = ReadDword(L"AutoCheck", &auto_check);
          ReadDword(L"PinyinScheme", &scheme);
          flutter::EncodableMap values;
          values[flutter::EncodableValue("scheme")] =
              flutter::EncodableValue(static_cast<int32_t>(scheme == 1 ? 1 : 0));
          values[flutter::EncodableValue("autoCheck")] =
              has_auto_check ? flutter::EncodableValue(auto_check != 0)
                             : flutter::EncodableValue();
          DWORD statistics_enabled = 1;
          ReadDword(L"StatisticsEnabled", &statistics_enabled);
          values[flutter::EncodableValue("statisticsEnabled")] =
              flutter::EncodableValue(statistics_enabled != 0);
          auto version = ReadInstalledString(L"Version");
          values[flutter::EncodableValue("version")] =
              flutter::EncodableValue(Utf8(version.empty() ? L"开发版本" : version));
          values[flutter::EncodableValue("directory")] =
              flutter::EncodableValue(Utf8(ReadInstalledString(L"ActiveDir")));
          DWORD enabled_packs = 0;
          ReadDword(L"EnabledDictionaryPacks", &enabled_packs);
          values[flutter::EncodableValue("enabledDictionaryPacks")] =
              flutter::EncodableValue(static_cast<int32_t>(enabled_packs & 0x7f));
          result->Success(flutter::EncodableValue(values));
          return;
        }
        if (method == "setDictionaryPack") {
          const auto* values = std::get_if<flutter::EncodableMap>(call.arguments());
          if (!values) {
            result->Error("invalid_argument", "Dictionary pack request is missing");
            return;
          }
          const auto id = values->find(flutter::EncodableValue("index"));
          const auto enabled = values->find(flutter::EncodableValue("enabled"));
          const auto* index = id == values->end() ? nullptr : std::get_if<int32_t>(&id->second);
          const auto* value = enabled == values->end() ? nullptr : std::get_if<bool>(&enabled->second);
          if (!index || *index < 0 || *index >= 7 || !value) {
            result->Error("invalid_argument", "Invalid dictionary pack");
            return;
          }
          DWORD mask = 0;
          DWORD generation = 0;
          ReadDword(L"EnabledDictionaryPacks", &mask);
          ReadDword(L"DictionaryGeneration", &generation);
          const DWORD bit = 1u << *index;
          mask = *value ? mask | bit : mask & ~bit;
          if (!WriteDword(L"EnabledDictionaryPacks", mask) ||
              !WriteDword(L"DictionaryGeneration", generation + 1)) {
            result->Error("registry", "Could not save dictionary preference");
          } else {
            result->Success(flutter::EncodableValue(static_cast<int32_t>(mask)));
          }
          return;
        }
        if (method == "installUpdate") {
          const auto* value = std::get_if<std::string>(call.arguments());
          const auto path = value ? Utf16(*value) : std::wstring();
          if (path.size() < 4 ||
              _wcsicmp(path.c_str() + path.size() - 4, L".exe") != 0 ||
              GetFileAttributesW(path.c_str()) == INVALID_FILE_ATTRIBUTES) {
            result->Error("invalid_argument", "Invalid installer path");
            return;
          }
          if (installer_process) {
            result->Error("busy", "An installer is already running");
            return;
          }
          const std::wstring parameters =
              L"/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /NOCLOSEAPPLICATIONS "
              L"/NORESTARTAPPLICATIONS /RESTARTEXITCODE=3010";
          SHELLEXECUTEINFOW info{};
          info.cbSize = sizeof(info);
          info.fMask = SEE_MASK_NOCLOSEPROCESS;
          info.hwnd = window;
          info.lpVerb = L"runas";
          info.lpFile = path.c_str();
          info.lpParameters = parameters.c_str();
          info.nShow = SW_HIDE;
          if (!ShellExecuteExW(&info) || !info.hProcess) {
            result->Error("install", "Could not start the elevated installer");
          } else {
            installer_process = info.hProcess;
            result->Success();
          }
          return;
        }
        if (method == "installerStatus") {
          if (!installer_process) {
            result->Error("install", "No installer is running");
            return;
          }
          DWORD code = STILL_ACTIVE;
          if (!GetExitCodeProcess(installer_process, &code)) {
            result->Error("install", "Could not read installer status");
          } else if (code == STILL_ACTIVE) {
            result->Success();
          } else {
            CloseHandle(installer_process);
            installer_process = nullptr;
            result->Success(flutter::EncodableValue(static_cast<int32_t>(code)));
          }
          return;
        }
        if (method == "setScheme") {
          const auto* value = std::get_if<int32_t>(call.arguments());
          if (!value || (*value != 0 && *value != 1)) {
            result->Error("invalid_argument", "Invalid Pinyin scheme");
          } else if (!WriteDword(L"PinyinScheme", *value)) {
            result->Error("registry", "Could not save Pinyin scheme");
          } else {
            result->Success();
          }
          return;
        }
        if (method == "setAutoCheck") {
          const auto* value = std::get_if<bool>(call.arguments());
          if (!value) {
            result->Error("invalid_argument", "Invalid update preference");
          } else if (!WriteDword(L"AutoCheck", *value ? 1 : 0)) {
            result->Error("registry", "Could not save update preference");
          } else {
            result->Success();
          }
          return;
        }
        if (method == "setStatisticsEnabled") {
          const auto* value = std::get_if<bool>(call.arguments());
          if (!value) {
            result->Error("invalid_argument", "Invalid statistics preference");
          } else if (!WriteDword(L"StatisticsEnabled", *value ? 1 : 0)) {
            result->Error("registry", "Could not save statistics preference");
          } else {
            result->Success();
          }
          return;
        }
        bool opened = false;
        if (method == "openReleaseNotes") {
          const auto* value = std::get_if<std::string>(call.arguments());
          const std::string prefix =
              "https://github.com/L-Chris/retype/releases/";
          opened = value && value->rfind(prefix, 0) == 0 &&
                   OpenPath(Utf16(*value));
        } else if (method == "openFeedback") {
          opened = OpenPath(L"https://github.com/L-Chris/retype/issues");
        } else if (method == "openLicense" || method == "openNotice") {
          const auto directory = ReadInstalledString(L"ActiveDir");
          if (!directory.empty()) {
            opened = OpenPath(directory +
                              (method == "openLicense" ? L"\\LICENSE"
                                                       : L"\\NOTICE.txt"));
          }
        } else {
          result->NotImplemented();
          return;
        }
        if (opened) {
          result->Success();
        } else {
          result->Error("launch", "Could not open the requested destination");
        }
      });
  return channel;
}
