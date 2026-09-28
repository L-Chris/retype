#include "settings_channel.h"

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

bool OpenPath(const std::wstring& path) {
  return reinterpret_cast<INT_PTR>(
             ShellExecuteW(nullptr, L"open", path.c_str(), nullptr, nullptr,
                           SW_SHOWNORMAL)) > 32;
}

bool OpenUpdates() {
  const auto directory = ReadInstalledString(L"ActiveDir");
  if (directory.empty()) return false;
  const std::wstring script = directory + L"\\update-ui.ps1";
  if (GetFileAttributesW(script.c_str()) == INVALID_FILE_ATTRIBUTES) return false;
  const std::wstring args =
      L"-NoProfile -STA -WindowStyle Hidden -ExecutionPolicy Bypass -File \"" +
      script + L"\"";
  return reinterpret_cast<INT_PTR>(ShellExecuteW(
             nullptr, L"open", L"powershell.exe", args.c_str(), nullptr,
             SW_HIDE)) > 32;
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
          PostMessageW(window, WM_CLOSE, 0, 0);
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
          auto version = ReadInstalledString(L"Version");
          values[flutter::EncodableValue("version")] =
              flutter::EncodableValue(Utf8(version.empty() ? L"开发版本" : version));
          result->Success(flutter::EncodableValue(values));
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
        bool opened = false;
        if (method == "openUpdates") {
          opened = OpenUpdates();
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
