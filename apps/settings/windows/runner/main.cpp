#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>
#include <windows.h>

#include "flutter_window.h"
#include "utils.h"

int APIENTRY wWinMain(_In_ HINSTANCE instance, _In_opt_ HINSTANCE prev,
                      _In_ wchar_t *command_line, _In_ int show_command) {
  constexpr wchar_t kTitle[] = L"retype 设置";
  HANDLE single_instance = CreateMutexW(nullptr, FALSE, L"Local\\retype-settings");
  if (!single_instance) return EXIT_FAILURE;
  if (GetLastError() == ERROR_ALREADY_EXISTS) {
    // The first process may still be creating its Flutter window.
    for (int attempt = 0; attempt < 20; ++attempt) {
      HWND existing = FindWindowW(L"FLUTTER_RUNNER_WIN32_WINDOW", kTitle);
      if (existing) {
        ShowWindow(existing, SW_RESTORE);
        SetForegroundWindow(existing);
        if (wcsstr(command_line, L"--updates")) {
          PostMessageW(existing, kShowUpdatesMessage, 0, 0);
        }
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

  flutter::DartProject project(L"data");

  std::vector<std::string> command_line_arguments =
      GetCommandLineArguments();

  project.set_dart_entrypoint_arguments(std::move(command_line_arguments));

  FlutterWindow window(project);
  Win32Window::Size size(960, 640);
  Win32Window::Point origin((GetSystemMetrics(SM_CXSCREEN) - size.width) / 2,
                           (GetSystemMetrics(SM_CYSCREEN) - size.height) / 2);
  if (!window.Create(kTitle, origin, size)) {
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
