package io.github.retype.ime

import android.text.InputType
import android.view.inputmethod.EditorInfo

data class EditorPolicy(
    val password: Boolean,
    val learning: Boolean,
    val literal: Boolean,
    val ascii: Boolean,
) {
    companion object {
        fun from(info: EditorInfo): EditorPolicy {
            val kind = info.inputType and InputType.TYPE_MASK_CLASS
            val variation = info.inputType and InputType.TYPE_MASK_VARIATION
            val password =
                (kind == InputType.TYPE_CLASS_TEXT &&
                    variation in
                        setOf(
                            InputType.TYPE_TEXT_VARIATION_PASSWORD,
                            InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD,
                            InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD,
                        )) ||
                    (kind == InputType.TYPE_CLASS_NUMBER &&
                        variation == InputType.TYPE_NUMBER_VARIATION_PASSWORD)
            val literal =
                password ||
                    kind == InputType.TYPE_CLASS_NUMBER ||
                    kind == InputType.TYPE_CLASS_PHONE ||
                    kind == InputType.TYPE_CLASS_DATETIME ||
                    info.inputType == InputType.TYPE_NULL
            val ascii =
                literal ||
                    (kind == InputType.TYPE_CLASS_TEXT &&
                        variation in
                            setOf(
                                InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS,
                                InputType.TYPE_TEXT_VARIATION_WEB_EMAIL_ADDRESS,
                                InputType.TYPE_TEXT_VARIATION_URI,
                            )) ||
                    info.imeOptions and EditorInfo.IME_FLAG_FORCE_ASCII != 0
            return EditorPolicy(
                password,
                !password && info.imeOptions and EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING == 0,
                literal,
                ascii,
            )
        }
    }
}
