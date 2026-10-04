package io.github.retype.ime

import android.app.Activity
import android.os.Bundle
import android.text.InputType
import android.widget.EditText
import android.widget.LinearLayout

/** Isolated editors: integration tests never type into personal apps. */
class EditorFixtureActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val column = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        column.setOnApplyWindowInsetsListener { view, insets ->
            view.setPadding(
                24,
                insets.systemWindowInsetTop + 24,
                24,
                insets.systemWindowInsetBottom + 24,
            )
            insets
        }
        listOf(
                "normal" to InputType.TYPE_CLASS_TEXT,
                "password" to (InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD),
                "email" to
                    (InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS),
            )
            .forEach { (name, type) ->
                column.addView(
                    EditText(this).apply {
                        contentDescription = "editor-$name"
                        hint = name
                        inputType = type
                    }
                )
            }
        setContentView(column)
    }
}
