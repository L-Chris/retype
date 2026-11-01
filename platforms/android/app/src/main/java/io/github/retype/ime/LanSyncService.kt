package io.github.retype.ime

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder

/** Keep enabled device connections running when the settings activity closes. */
class LanSyncService : Service() {
  override fun onCreate() {
    super.onCreate()
    foreground()
    LanClipboard.get(this).serviceRunning()
  }

  private fun foreground() {
    val manager = getSystemService(NotificationManager::class.java)
    manager.createNotificationChannel(NotificationChannel("clipboard-devices", "跨设备剪贴板", NotificationManager.IMPORTANCE_LOW))
    val settings = PendingIntent.getActivity(this, 0, Intent(this, SettingsActivity::class.java),
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    val stop = PendingIntent.getService(this, 1, Intent(this, LanSyncService::class.java).setAction("stop"),
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    val notification = Notification.Builder(this, "clipboard-devices")
        .setSmallIcon(android.R.drawable.stat_notify_sync).setContentTitle("跨设备剪贴板已开启")
        .setContentText("允许已关联设备连接和同步剪贴板").setContentIntent(settings).setOngoing(true)
        .addAction(Notification.Action.Builder(null, "关闭同步", stop).build()).build()
    if (Build.VERSION.SDK_INT >= 29) startForeground(50206, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
    else startForeground(50206, notification)
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    // Rapid disable/re-enable can reuse a ServiceRecord without another onCreate.
    foreground()
    LanClipboard.get(this).serviceRunning()
    if (intent?.action == "stop") LanClipboard.get(this).enable(false)
    if (!getSharedPreferences("lanClipboard", MODE_PRIVATE).getBoolean("enabled", false)) {
      stopSelf()
      return START_NOT_STICKY
    }
    LanClipboard.get(this).resume()
    return START_STICKY
  }

  override fun onDestroy() {
    LanClipboard.get(this).serviceStopped()
    super.onDestroy()
  }
  override fun onBind(intent: Intent?): IBinder? = null
}
