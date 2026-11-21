package com.hantekllc.iptvrs.host

import android.content.Context
import android.widget.FrameLayout
import kotlin.math.max
import kotlin.math.roundToInt

/**
 * Sizes and centers the video surface for a picture-size mode; whatever
 * exceeds the frame is clipped by it. Layout only (the SurfaceView is never
 * scaled or translated), so the video surface composites normally.
 */
internal class PictureFrame(context: Context) : FrameLayout(context) {
    var videoAspect = 16f / 9f
        set(value) {
            if (value > 0f && value != field) {
                field = value
                requestLayout()
            }
        }
    var mode = "normal"
        set(value) {
            if (value != field) {
                field = value
                requestLayout()
            }
        }

    override fun onMeasure(widthSpec: Int, heightSpec: Int) {
        val pw = MeasureSpec.getSize(widthSpec)
        val ph = MeasureSpec.getSize(heightSpec)
        setMeasuredDimension(pw, ph)
        if (pw <= 0 || ph <= 0) return
        val (w, h) = pictureSize(mode, pw.toFloat(), ph.toFloat(), videoAspect)
        for (i in 0 until childCount) {
            getChildAt(i).measure(
                MeasureSpec.makeMeasureSpec(w, MeasureSpec.EXACTLY),
                MeasureSpec.makeMeasureSpec(h, MeasureSpec.EXACTLY),
            )
        }
    }

    override fun onLayout(changed: Boolean, left: Int, top: Int, right: Int, bottom: Int) {
        val pw = right - left
        val ph = bottom - top
        for (i in 0 until childCount) {
            val c = getChildAt(i)
            val x = (pw - c.measuredWidth) / 2
            val y = (ph - c.measuredHeight) / 2
            c.layout(x, y, x + c.measuredWidth, y + c.measuredHeight)
        }
    }
}

/** The surface size for `mode` in a `pw`×`ph` frame showing video of aspect `va`. */
internal fun pictureSize(mode: String, pw: Float, ph: Float, va: Float): Pair<Int, Int> {
    val pa = pw / ph
    fun fit(a: Float) = if (a > pa) pw to pw / a else ph * a to ph
    fun cover(a: Float) = if (a > pa) ph * a to ph else pw to pw / a
    fun scaled(p: Pair<Float, Float>, k: Float) = p.first * k to p.second * k
    val (w, h) = when (mode) {
        "full" -> pw to ph
        "zoom" -> cover(va)
        // A wide film letterboxed in the video: zoom until it fills the frame.
        "cinema" -> scaled(fit(va), max(1f, 2.35f / va))
        "reduced" -> scaled(fit(va), 0.85f)
        "4:3" -> fit(4f / 3f)
        "16:9" -> fit(16f / 9f)
        "2.35:1" -> fit(2.35f)
        else -> fit(va)
    }
    return w.roundToInt() to h.roundToInt()
}
