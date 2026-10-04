package io.github.retype.ime

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.*
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

/** Explicitly opted-in diagnostics; ordinary test runs never access a user's cloud. */
@RunWith(AndroidJUnit4::class)
class SavedCloudTest {
    @Test
    fun testSavedConnectionAndSync() = runBlocking {
        assumeTrue(InstrumentationRegistry.getArguments().getString("realCloud") == "true")
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val store = AppStore(context)
        val before = store.cloud().toString()
        val beforeStatus = store.prefs.getString("syncStatus", null)
        assertTrue("Saved cloud sync must be enabled", store.cloud().optBoolean("enabled"))
        val connection = CloudSync.run(context, test = true)
        println("Cloud connection: ${connection.getString("message")}")
        assertEquals(before, store.cloud().toString())
        assertEquals(beforeStatus, store.prefs.getString("syncStatus", null))
        val result = CloudSync.run(context)
        println("Cloud sync: ${result.getString("message")}; awaitingMerge=${result.optBoolean("awaitingMerge")}; conflicts=${result.optJSONArray("conflicts")?.length() ?: 0}")
        assertEquals(before, store.cloud().toString())
        assertFalse("Initial merge needs explicit confirmation", result.optBoolean("awaitingMerge"))
        assertTrue("A completed sync must return settings", result.has("values"))
        println("Synced setting fields: ${result.getJSONObject("values").length()}")
    }

    @Test
    fun connectionTestPreservesUnsavedForm() {
        assumeTrue(InstrumentationRegistry.getArguments().getString("realCloud") == "true")
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val store = AppStore(context)
        val before = store.cloud()
        val device = UiDevice.getInstance(instrumentation)
        fun find(selector: BySelector, timeout: Long = 10000): UiObject2 {
            val deadline = android.os.SystemClock.uptimeMillis() + timeout
            do {
                if (android.os.Build.VERSION.SDK_INT >= 33) instrumentation.uiAutomation.clearCache()
                device.findObject(selector)?.let { return it }
                android.os.SystemClock.sleep(100)
            } while (android.os.SystemClock.uptimeMillis() < deadline)
            error("Expected settings control was not found")
        }
        try {
            device.executeShellCommand("am start -W -f 0x10008000 -n io.github.retype.ime/.SettingsActivity")
            find(By.text("云同步")).click()
            val toggle = find(By.checkable(true))
            assertTrue(toggle.isChecked)
            toggle.click() // Persist disabled, then enable as an unsaved draft.
            find(By.checkable(true)).click()
            find(By.text(before.getString("device_name"))).text = "sync-draft-regression"
            find(By.text("测试连接")).click()
            find(By.text("连接成功"), 60000)
            assertTrue("Connection test must preserve the enabled draft", find(By.checkable(true)).isChecked)
            assertNotNull(find(By.text("sync-draft-regression")))
            assertFalse("Testing must not save the enabled draft", store.cloud().optBoolean("enabled"))
        } finally {
            store.saveCloud(before)
            CloudSync.schedule(context)
            device.executeShellCommand("am start -W -f 0x10008000 -n io.github.retype.ime/.SettingsActivity")
        }
    }
}
