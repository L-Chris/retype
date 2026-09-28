#include "flutter_window.h"

#include <optional>
#include <windowsx.h>

#include "flutter/generated_plugin_registrant.h"
#include "settings_channel.h"

FlutterWindow::FlutterWindow(const flutter::DartProject& project)
    : project_(project) {}

FlutterWindow::~FlutterWindow() {}

bool FlutterWindow::OnCreate() {
  if (!Win32Window::OnCreate()) {
    return false;
  }

  RECT frame = GetClientArea();

  // The size here must match the window dimensions to avoid unnecessary surface
  // creation / destruction in the startup path.
  flutter_controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project_);
  // Ensure that basic setup of the controller was successful.
  if (!flutter_controller_->engine() || !flutter_controller_->view()) {
    return false;
  }
  RegisterPlugins(flutter_controller_->engine());
  settings_channel_ = RegisterSettingsChannel(flutter_controller_->engine(), GetHandle());
  SetChildContent(flutter_controller_->view()->GetNativeWindow());

  flutter_controller_->engine()->SetNextFrameCallback([&]() {
    this->Show();
  });

  // Flutter can complete the first frame before the "show window" callback is
  // registered. The following call ensures a frame is pending to ensure the
  // window is shown. It is a no-op if the first frame hasn't completed yet.
  flutter_controller_->ForceRedraw();

  return true;
}

void FlutterWindow::OnDestroy() {
  settings_channel_.reset();
  if (flutter_controller_) {
    flutter_controller_ = nullptr;
  }

  Win32Window::OnDestroy();
}

LRESULT
FlutterWindow::MessageHandler(HWND hwnd, UINT const message,
                              WPARAM const wparam,
                              LPARAM const lparam) noexcept {
  // Briefly reuse the engine for repeated settings visits, then release it.
  // WM_CLOSE still destroys the process for installers and system shutdown.
  constexpr UINT_PTR kIdleTimer = 42;
  if (message == kHideSettingsMessage) {
    ShowWindow(hwnd, SW_HIDE);
    SetTimer(hwnd, kIdleTimer, 60000, nullptr);
    return 0;
  }
  if (message == WM_SHOWWINDOW && wparam) KillTimer(hwnd, kIdleTimer);
  if (message == WM_TIMER && wparam == kIdleTimer) {
    KillTimer(hwnd, kIdleTimer);
    if (!IsWindowVisible(hwnd)) PostMessageW(hwnd, WM_CLOSE, 0, 0);
    return 0;
  }
  if (message == kShowUpdatesMessage && settings_channel_) {
    settings_channel_->InvokeMethod("showUpdates", nullptr);
    return 0;
  }
  if (message == WM_NCCALCSIZE && wparam) {
    return 0;  // Flutter owns the entire frame, including the former title area.
  }
  // Give Flutter, including plugins, an opportunity to handle window messages.
  if (flutter_controller_) {
    std::optional<LRESULT> result =
        flutter_controller_->HandleTopLevelWindowProc(hwnd, message, wparam,
                                                      lparam);
    if (result) {
      return *result;
    }
  }

  switch (message) {
    case WM_NCHITTEST: {
      if (IsZoomed(hwnd)) break;
      RECT bounds{};
      GetWindowRect(hwnd, &bounds);
      const int edge = MulDiv(7, GetDpiForWindow(hwnd), 96);
      const int x = GET_X_LPARAM(lparam);
      const int y = GET_Y_LPARAM(lparam);
      const bool left = x < bounds.left + edge;
      const bool right = x >= bounds.right - edge;
      const bool top = y < bounds.top + edge;
      const bool bottom = y >= bounds.bottom - edge;
      if (top && left) return HTTOPLEFT;
      if (top && right) return HTTOPRIGHT;
      if (bottom && left) return HTBOTTOMLEFT;
      if (bottom && right) return HTBOTTOMRIGHT;
      if (left) return HTLEFT;
      if (right) return HTRIGHT;
      if (top) return HTTOP;
      if (bottom) return HTBOTTOM;
      break;
    }
    case WM_GETMINMAXINFO: {
      auto* bounds = reinterpret_cast<MINMAXINFO*>(lparam);
      const UINT dpi = GetDpiForWindow(hwnd);
      bounds->ptMinTrackSize.x = MulDiv(760, dpi, 96);
      bounds->ptMinTrackSize.y = MulDiv(520, dpi, 96);
      return 0;
    }
    case WM_FONTCHANGE:
      flutter_controller_->engine()->ReloadSystemFonts();
      break;
  }

  return Win32Window::MessageHandler(hwnd, message, wparam, lparam);
}
