package com.hantekllc.iptvrs.host

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DocumentPickersTest {
    @Test fun noResolvingActivityIsNoPicker() {
        assertFalse(DocumentPickers.usable(emptyList()))
        assertFalse(DocumentPickers.usable(listOf(null)))
    }

    @Test fun theTvFrameworkStubsAreNoPicker() {
        assertFalse(DocumentPickers.usable(listOf("com.android.tv.frameworkpackagestubs")))
        assertFalse(DocumentPickers.usable(listOf("com.android.tv.frameworkpackagestubs", null)))
    }

    @Test fun anyOtherResolvingPackageIsAPicker() {
        assertTrue(DocumentPickers.usable(listOf("com.android.documentsui")))
        assertTrue(DocumentPickers.usable(listOf("com.android.tv.frameworkpackagestubs", "com.example.files")))
    }

    @Test fun theLaunchSkipsTheStubForTheBestRealPicker() {
        assertEquals(
            "com.example.files",
            DocumentPickers.choose(
                listOf("com.android.tv.frameworkpackagestubs", null, "com.example.files", "com.example.other"),
            ),
        )
        assertNull(DocumentPickers.choose(listOf("com.android.tv.frameworkpackagestubs")))
    }
}
