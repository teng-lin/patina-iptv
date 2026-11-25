package com.hantekllc.iptvrs.host

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager

/** The TV launcher shares the host without declaring handheld picture in picture. */
class LeanbackActivity : TvActivity()

/** Notifications return to the same activity as this device's launcher. */
internal fun appLaunchIntent(context: Context): Intent =
    Intent(
        context,
        launcherActivity(context.packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK)),
    )

internal fun launcherActivity(tv: Boolean): Class<out TvActivity> = if (tv) LeanbackActivity::class.java else TvActivity::class.java

internal fun redirectedActivity(
    current: Class<out TvActivity>,
    tv: Boolean,
): Class<out TvActivity>? = launcherActivity(tv).takeUnless { it == current }
