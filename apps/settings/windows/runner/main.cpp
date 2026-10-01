#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>
#include <windows.h>
#include <cwchar>

#include "flutter_window.h"
#include "utils.h"
#include "startup_diagnostics.h"

int APIENTRY wWinMain(_In_ HINSTANCE instance, _In_opt_ HINSTANCE prev,
                      _In_ wchar_t *command_line, _In_ int show_command) {
  constexpr wchar_t kTitle[] = L"retype 设置";
  if (auto argument = wcsstr(command_line, L"--opened-at=")) {
    settings_startup::opened_at = wcstoul(argument + 12, nullptr, 10);
  }
  settings_startup::Log("process.entry");
  if (wcsstr(command_line, L"--open-reason=version_mismatch")) {
    settings_startup::Log("launcher.version_mismatch");
  } else if (wcsstr(command_line, L"--open-reason=path_query_failed")) {
    settings_startup::Log("launcher.path_query_failed");
  } else if (wcsstr(command_line, L"--open-reason=process_query_failed")) {
    settings_startup::Log("launcher.process_query_failed");
  } else if (wcsstr(command_line, L"--open-reason=no_window")) {
    settings_startup::Log("launcher.no_window");
  }
  const auto executable = settings_startup::ExecutablePath();
  const auto instance_name = settings_startup::InstanceName(executable);
  HANDLE single_instance = CreateMutexW(nullptr, FALSE, instance_name.c_str());
  if (!single_instance) return EXIT_FAILURE;
  if (GetLastError() == ERROR_ALREADY_EXISTS) {
    // The first process may still be creating its Flutter window.
    for (int attempt = 0; attempt < 20; ++attempt) {
      HWND existing = settings_startup::FindSameVersionWindow(executable);
      if (existing) {
        PostMessageW(existing, kActivateSettingsMessage, settings_startup::opened_at, 0);
        ShowWindow(existing, SW_RESTORE);
        SetForegroundWindow(existing);
        if (wcsstr(command_line, L"--updates")) {
          PostMessageW(existing, kShowUpdatesMessage, 0, 0);
        }
        settings_startup::Log("reuse.forwarded");
        break;
      }
      Sleep(100);
    }
    CloseHandle(single_instance);
    return EXIT_SUCCESS;
  }
  // Attach to console when present (e.g., 'flutter run') or create a
  // new console when running with a debugger.
  if (!::AttachConsole(ATTACH_PARENT_PROCESS) && ::IsDebuggerPresent()) {
    CreateAndAttachConsole();
  }

  // Initialize COM, so that it is available for use in the library and/or
  // plugins.
  ::CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
  settings_startup::Log("process.com_ready");

  flutter::DartProject project(L"data");

  std::vector<std::string> command_line_arguments =
      GetCommandLineArguments();

  project.set_dart_entrypoint_arguments(std::move(command_line_arguments));

  FlutterWindow window(project);
  Win32Window::Size size(960, 640);
  if (!window.Create(kTitle, size)) {
    ::CoUninitialize();
    CloseHandle(single_instance);
    return EXIT_FAILURE;
  }
  window.SetQuitOnClose(true);

  ::MSG msg;
  while (::GetMessage(&msg, nullptr, 0, 0)) {
    ::TranslateMessage(&msg);
    ::DispatchMessage(&msg);
  }

  ::CoUninitialize();
  CloseHandle(single_instance);
  return EXIT_SUCCESS;
}
