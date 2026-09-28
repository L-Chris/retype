#ifndef RUNNER_SETTINGS_CHANNEL_H_
#define RUNNER_SETTINGS_CHANNEL_H_

#include <flutter/encodable_value.h>
#include <flutter/method_channel.h>
#include <windows.h>

#include <memory>

namespace flutter {
class FlutterEngine;
}

std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>>
RegisterSettingsChannel(flutter::FlutterEngine* engine, HWND window);

#endif  // RUNNER_SETTINGS_CHANNEL_H_
