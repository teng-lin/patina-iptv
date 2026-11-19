package com.hantekllc.iptvrs.host

import android.os.Build
import android.view.View
import android.view.WindowInsets
import com.longbridge.nsh.NshComponent
import com.longbridge.nsh.NshComponentContext

/** A floating software keyboard can be visible with no insets. */
internal class ImeVisibilityComponent(
    context: NshComponentContext,
) : NshComponent {
    companion object {
        const val NAME = "tv.ime-visibility"
    }

    private var visible: Boolean? = null
    private val observer =
        object : View(context.context) {
            override fun onAttachedToWindow() {
                super.onAttachedToWindow()
                requestApplyInsets()
            }

            override fun onApplyWindowInsets(insets: WindowInsets): WindowInsets {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    val next = insets.isVisible(WindowInsets.Type.ime())
                    if (visible != next) {
                        visible = next
                        context.emit("visibility", next)
                    }
                }
                return super.onApplyWindowInsets(insets)
            }
        }.apply {
            importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO
            isFocusable = false
        }

    override fun view(): View = observer

    override fun setProp(
        name: String,
        value: Any?,
    ) = Unit
}
