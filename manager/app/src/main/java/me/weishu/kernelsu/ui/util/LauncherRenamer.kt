package me.weishu.kernelsu.ui.util

import android.content.Context
import android.os.Build
import com.android.apksig.ApkSigner
import com.android.tools.build.apkzlib.zip.AlignmentRules
import com.android.tools.build.apkzlib.zip.ZFile
import com.android.tools.build.apkzlib.zip.ZFileOptions
import java.io.ByteArrayInputStream
import java.io.File
import java.nio.charset.StandardCharsets
import java.security.KeyStore
import java.security.PrivateKey
import java.security.SecureRandom
import java.security.cert.X509Certificate

/**
 * Gives a launcher APK a package name of its own, on the device.
 *
 * A package name is baked into a signed APK, so "a fresh identity on every install" cannot be
 * picked from a build-time pool: the APK has to be rewritten and signed again right here.
 *
 * Two facts make that cheap rather than clever:
 *
 *  - the name appears in exactly two places — `AndroidManifest.xml`'s string pool and
 *    `resources.arsc`'s package chunk — and each is a length-prefixed UTF-16 string, so a
 *    replacement of the *same length* keeps every offset in both files valid;
 *  - the launcher's signing key ships in the Manager's assets, so the Manager can sign what it
 *    just rewrote.
 *
 * Five-letter segments are what keep a generated name exactly as long as the one in the APK
 * (and keep it a valid Java identifier). Both are checked here rather than assumed: anything
 * unexpected fails the rename, and the caller installs the untouched variant instead.
 */
object LauncherRenamer {
    private const val KEYSTORE_ASSET = "calculator-signing.jks"
    private const val STORE_PASSWORD = "kernelsu123"
    private const val KEY_ALIAS = "kernelsu"
    private const val KEY_PASSWORD = "kernelsu123"
    private const val SEGMENT_LENGTH = 5
    private const val RESOURCE_TABLE = "resources.arsc"
    private const val ALPHABET = "abcdefghijklmnopqrstuvwxyz"
    private val NAMED_ENTRIES = arrayOf("AndroidManifest.xml", RESOURCE_TABLE)

    /** `com.xxxxx.yyyyy`: the shape the launcher APKs are built with. */
    fun randomPackageName(): String {
        val random = SecureRandom()
        return "com.${letters(random)}.${letters(random)}"
    }

    private fun letters(random: SecureRandom) = buildString(SEGMENT_LENGTH) {
        repeat(SEGMENT_LENGTH) { append(ALPHABET[random.nextInt(ALPHABET.length)]) }
    }

    /**
     * Writes [source] to [target] with `from` replaced by `to` and a fresh signature.
     *
     * Returns false — leaving [target] absent — when the APK does not carry that name exactly
     * once, when the lengths differ, or when anything else goes wrong. The caller keeps using
     * the variant it was given then; a generated name is a nicety, not a prerequisite.
     */
    fun rename(context: Context, source: File, target: File, from: String, to: String): Boolean =
        runCatching {
            require(from.length == to.length) { "包名长度必须保持不变" }
            val fromBytes = from.toByteArray(StandardCharsets.UTF_16LE)
            val toBytes = to.toByteArray(StandardCharsets.UTF_16LE)

            val work = File(target.parentFile, "${target.name}.work")
            work.delete()
            target.delete()
            source.copyTo(work, overwrite = true)
            val options = ZFileOptions().apply {
                setAlignmentRule(
                    AlignmentRules.compose(
                        AlignmentRules.constantForSuffix(".arsc", 4),
                        AlignmentRules.constantForSuffix(".so", 16384),
                    ),
                )
            }
            ZFile.openReadWrite(work, options).use { zip ->
                for (name in NAMED_ENTRIES) {
                    val entry = zip.get(name) ?: return@runCatching false
                    val patched = replaceOnce(entry.read(), fromBytes, toBytes) ?: return@runCatching false
                    // The resource table has to stay uncompressed and 4-byte aligned: Android 11+
                    // refuses to install an APK whose resources.arsc is deflated.
                    zip.add(name, ByteArrayInputStream(patched), name != RESOURCE_TABLE)
                }
                zip.update()
            }

            context.assets.open(KEYSTORE_ASSET).use { stream ->
                val keystore = KeyStore.getInstance("PKCS12")
                    .apply { load(stream, STORE_PASSWORD.toCharArray()) }
                val key = keystore.getKey(KEY_ALIAS, KEY_PASSWORD.toCharArray()) as PrivateKey
                val chain = keystore.getCertificateChain(KEY_ALIAS).map { it as X509Certificate }
                ApkSigner.Builder(listOf(ApkSigner.SignerConfig.Builder(KEY_ALIAS, key, chain).build()))
                    .setInputApk(work)
                    .setOutputApk(target)
                    // minSdk is 31, so v1 has nothing to do; v2 is what the assets ship with.
                    .setV1SigningEnabled(false)
                    .setV2SigningEnabled(true)
                    .setV3SigningEnabled(true)
                    .setMinSdkVersion(Build.VERSION_CODES.S)
                    .build()
                    .sign()
            }

            work.delete()
            true
        }.getOrDefault(false)

    private fun replaceOnce(data: ByteArray, from: ByteArray, to: ByteArray): ByteArray? {
        val at = indexOf(data, from)
        if (at < 0) return null
        if (indexOf(data, from, at + 1) >= 0) return null
        return data.copyOf().also { System.arraycopy(to, 0, it, at, to.size) }
    }

    private fun indexOf(haystack: ByteArray, needle: ByteArray, from: Int = 0): Int {
        outer@ for (i in maxOf(from, 0)..haystack.size - needle.size) {
            for (j in needle.indices) {
                if (haystack[i + j] != needle[j]) continue@outer
            }
            return i
        }
        return -1
    }
}
