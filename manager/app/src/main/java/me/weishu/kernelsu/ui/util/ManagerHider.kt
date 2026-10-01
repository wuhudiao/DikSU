package me.weishu.kernelsu.ui.util

import android.content.Context
import android.util.Log
import android.os.Build
import android.os.Process
import com.android.apksig.ApkSigner
import com.android.tools.build.apkzlib.zip.AlignmentRules
import com.android.tools.build.apkzlib.zip.ZFile
import com.android.tools.build.apkzlib.zip.ZFileOptions
import java.io.ByteArrayInputStream
import java.io.File
import java.nio.charset.StandardCharsets
import java.security.KeyStore
import java.security.Security
import java.security.PrivateKey
import java.security.SecureRandom
import java.security.cert.X509Certificate
import me.weishu.kernelsu.BuildConfig
import me.weishu.kernelsu.Natives
import me.weishu.kernelsu.R

/**
 * Installs the Manager under a package name picked on the device — KernelSU's answer to Magisk's
 * "hide the app".
 *
 * The name is baked into a signed APK, so unlike Magisk's stub the Manager cannot simply build a
 * second APK: it rewrites its own, right here. It also cannot pick a fresh signing key the way
 * Magisk does: KernelSU crowns its manager by the APK's signing certificate
 * (`kernel/manager/apk_sign.c`, reached through `throne_tracker`), so the rewritten APK has to
 * carry the very key this build was signed with. That key ships in the Manager's assets.
 *
 * What has to be rewritten, measured on a real build of this APK:
 *
 *   AndroidManifest.xml    UTF-16LE x5   package, the dynamic-receiver permission, the two
 *                                        authorities, WebUI's taskAffinity
 *   resources.arsc         UTF-16LE x1   the package chunk's name, in its fixed-width field
 *   lib/<abi>/libksud.so   UTF-8    x2   ksud's own default package name
 *
 * The manifest's component names name Java classes, not the package, so they stay, as does
 * everything in classes.dex — which is exactly why the build hands the Manager a package name of its
 * own and those components live in a package of their own (`com.mngr.app.*`); the spots that used to
 * fold in `BuildConfig.APPLICATION_ID` read `context.packageName` instead.
 *
 * A generated name is as long as the one it replaces and every replacement keeps its byte count,
 * so no offset inside either rewritten file moves.
 */
object ManagerHider {
    private const val TAG = "KernelSU"

    /**
     * The strings Android and AGP derive from the package name, identified by what follows it. The
     * empty suffix is the package attribute itself.
     */
    private val PACKAGE_NAME_SUFFIXES = listOf(
        "",
        ".DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION",
        ".androidx-startup",
        ".fileprovider",
        ".webui",
    )

    private const val ANDROID_MANIFEST = "AndroidManifest.xml"
    private const val RESOURCE_TABLE = "resources.arsc"
    private const val KEYSTORE_ASSET = "kernelsu.jks"
    private const val ALPHABET = "abcdefghijklmnopqrstuvwxyz"

    /** The ABI folders a Manager build can carry ksud in. */
    private val ABIS = listOf("arm64-v8a", "armeabi-v7a", "x86", "x86_64", "riscv64")

    /**
     * Somewhere that outlives this package: the chain below uninstalls it, and its own directory
     * goes with it. /data/local/tmp is also where an `adb install` stages an APK, so the installer
     * can read what is put there.
     */
    private const val STAGED_DIR = "/data/local/tmp"

    /** Where the two halves of a hide report what they did. */
    private const val LOG_PATH = "$STAGED_DIR/ksu-hide.log"

    /** What the old install tells the renamed copy that has to finish the job. */
    const val EXTRA_HIDDEN_FROM = "me.weishu.kernelsu.extra.HIDDEN_FROM"
    const val EXTRA_HIDDEN_APK = "me.weishu.kernelsu.extra.HIDDEN_APK"
    const val EXTRA_HIDDEN_ORIGINAL = "me.weishu.kernelsu.extra.HIDDEN_ORIGINAL"

    /** Where a hidden install keeps the plain APK, so it can put the user back later. */
    private const val ORIGINAL_APK = "ksu-original.apk"

/** A component name is the Java class, which renaming does not touch — and which is no longer the
 *  package's own name, so it has to be written out. */
private const val MAIN_COMPONENT = "com.mngr.app.ui.MainActivity"

    /**
     * Repacks this Manager under a fresh name, installs that copy, and hands the rest of the job to
     * it. Returns the name it installed as, or null when something said no.
     *
     * Two facts shape this dance. The kernel only looks for a manager when it has just lost the one
     * it crowned (`manager/throne_tracker.c`): while the crowned package is still in packages.list
     * it does nothing, and the moment that package disappears it clears the seat and waits for the
     * *next* package event to search. And uninstalling a package kills everything in its process
     * group, so no shell of ours survives long enough to supply that next event.
     *
     * So the renamed copy finishes it, and to be able to it is granted root first — this app is the
     * crowned manager, and granting a uid root is what the Superuser screen does anyway. The copy
     * is then launched with the two things it needs to know: which package to drop, and where its
     * own APK is staged.
     */
    fun hide(context: Context, name: String?, icon: ByteArray?): String? {
        val from = context.packageName
        val to = randomPackageName(from.length)
        val source = File(context.applicationInfo.sourceDir)
        val work = File(context.cacheDir, "hidden-work.apk")
        val repacked = File(context.cacheDir, "hidden-$to.apk")
        if (!repack(context, source, work, repacked, from, to, name, icon)) return null

        val staged = "$STAGED_DIR/ksu-hidden-$to.apk"
        val original = "$STAGED_DIR/ksu-original-$to.apk"
        // Hide again from a copy that is already customized and the running APK is the customized
        // one — the plain Manager is the one kept in this app's files, and that is what a later
        // restore has to install.
        val plain = File(context.filesDir, ORIGINAL_APK).takeIf { it.exists() && it.length() > 0 } ?: source
        val stagedChain = buildString {
            append("echo \"hide $from -> $to\" > $LOG_PATH;")
            append("mkdir -p $STAGED_DIR && cp $repacked $staged && cp $plain $original && ")
            append("chmod 644 $staged $original || exit 1;")
            append("echo install-first >> $LOG_PATH;")
            append("pm install $staged >> $LOG_PATH 2>&1")
        }
        val installed = withNewRootShell { newJob().add(stagedChain).exec().isSuccess }
        if (!installed) return null

        val uid = newUid(to) ?: return null
        if (!grantRoot(to, uid)) return null

        launchWithHandOff(to, from, staged, original)
        return to
    }

    /**
     * Puts the plain Manager back: the copy of the original APK the hidden install kept is staged
     * and installed, and that install finishes the job the same way a hide does — it drops the
     * hidden package and reinstalls itself, which is the package event the kernel searches on.
     */
    fun restore(context: Context): Boolean {
        val stored = File(context.filesDir, ORIGINAL_APK)
        if (!stored.exists() || stored.length() == 0L) return false
        val from = context.packageName
        val to = BuildConfig.APPLICATION_ID
        if (to == from) return false
        val staged = "$STAGED_DIR/ksu-restore.apk"
        val stagedChain = buildString {
            append("echo \"restore $from -> $to\" > $LOG_PATH;")
            append("cp $stored $staged && chmod 644 $staged || exit 1;")
            append("echo install-original >> $LOG_PATH;")
            append("pm install $staged >> $LOG_PATH 2>&1")
        }
        val installed = withNewRootShell { newJob().add(stagedChain).exec().isSuccess }
        if (!installed) return false
        val uid = newUid(to) ?: return false
        if (!grantRoot(to, uid)) return false
        launchWithHandOff(to, from, staged, null)
        return true
    }

    /** Launches the freshly installed copy with the two things it needs to finish the job. */
    private fun launchWithHandOff(to: String, from: String, staged: String, original: String?) {
        val extras = StringBuilder()
            .append(" --es $EXTRA_HIDDEN_FROM $from")
            .append(" --es $EXTRA_HIDDEN_APK $staged")
        if (original != null) extras.append(" --es $EXTRA_HIDDEN_ORIGINAL $original")
        withNewRootShell {
            newJob().add(
                "am start -n $to/$MAIN_COMPONENT$extras >> $LOG_PATH 2>&1",
            ).exec()
        }
    }

    /** True when this install is a customized copy that kept the plain APK for a restore. */
    fun canRestore(context: Context) = File(context.filesDir, ORIGINAL_APK).let { it.exists() && it.length() > 0 }

    /**
     * The half the renamed copy runs: drop the package that holds the seat, then reinstall itself —
     * that update is the package event the kernel searches on, and by then it is the only
     * manager-signed APK left. Both steps need the root [hide] granted it, and its own shell is a
     * child of *this* app, which nothing is trying to remove.
     */
    fun finishHide(context: Context, previous: String, staged: String, original: String?) {
        // Keep the plain APK for a later restore. It is the un-customized build, so restoring gives
        // back the original name, icon and label rather than another copy of the customized one.
        runCatching {
            if (!original.isNullOrEmpty()) {
                File(original).takeIf { it.exists() }?.copyTo(File(context.filesDir, ORIGINAL_APK), overwrite = true)
            } else {
                File(context.filesDir, ORIGINAL_APK).delete()
            }
        }
        val chain = buildString {
            append("echo \"finish: drop $previous\" >> $LOG_PATH;")
            append("pm uninstall $previous >> $LOG_PATH 2>&1;")
            append("echo reinstall-self >> $LOG_PATH;")
            append("pm install -r $staged >> $LOG_PATH 2>&1;")
            append("echo finished >> $LOG_PATH;")
            append("rm -f $staged")
        }
        runCatching {
            withNewRootShell { newJob().add("sh -c '$chain' < /dev/null > /dev/null 2>&1").exec() }
        }
    }

    /** The uid the freshly installed copy got, asked of the package manager. */
    private fun newUid(pkg: String): Int? = runCatching {
        val lines = mutableListOf<String>()
        withNewRootShell { newJob().add("pm list packages -U | grep '^package:$pkg '").to(lines).exec() }
        lines.firstOrNull()
            ?.let { Regex("uid:(\\d+)").find(it)?.groupValues?.get(1)?.toIntOrNull() }
    }.getOrNull()

    /** Grants a uid root the way the Superuser screen does: by saving a profile with allowSu. */
    private fun grantRoot(pkg: String, uid: Int): Boolean = runCatching {
        val profile = Natives.getAppProfile(pkg, uid)
        Natives.setAppProfile(profile.copy(allowSu = true))
    }.getOrDefault(false)

    /**
     * Drops the grant [hide] handed the copy it created. Only the crowned manager can write an app
     * profile, so this is a no-op until the copy that calls it is the manager — which is exactly
     * when the grant has done its job: the manager has root by definition (`is_manager()`).
     */
    fun revokeTemporaryGrant(pkg: String) {
        runCatching {
            val profile = Natives.getAppProfile(pkg, Process.myUid())
            if (profile.allowSu) Natives.setAppProfile(profile.copy(allowSu = false))
        }
    }

    /**
     * `com.xxxxx.yyyyy`, exactly as long as the package name it replaces — the same shape the
     * build gives the Manager, so a hidden install can be hidden again.
     */
    fun randomPackageName(length: Int): String {
        val random = SecureRandom()
        val body = length - "com.".length
        fun letters(count: Int) = buildString(count) {
            repeat(count) { append(ALPHABET[random.nextInt(ALPHABET.length)]) }
        }
        // Two dot-separated words fit from seven letters up; anything shorter stays one word.
        return if (body >= 7) {
            val left = body / 2
            "com.${letters(left)}.${letters(body - left - 1)}"
        } else {
            "com.${letters(body)}"
        }
    }

    /** Copies [source] to [target] with the package name swapped and a fresh signature on it. */
    private fun repack(
        context: Context,
        source: File,
        work: File,
        target: File,
        from: String,
        to: String,
        name: String?,
        icon: ByteArray?,
    ): Boolean = try {
        require(from.length == to.length) { "a renamed package has to keep its byte count" }
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
            val replacements = mutableMapOf<String, ByteArray>()

            val manifest = zip.get(ANDROID_MANIFEST)?.read() ?: return false
            replacements[ANDROID_MANIFEST] =
                patchPackageName(manifest, from, to) ?: return false

            val resources = zip.get(RESOURCE_TABLE)?.read() ?: return false
            var table = patchResourceTable(resources, from, to) ?: return false
            if (name != null || icon != null) {
                val targets = mutableListOf<ResourcePatcher.Target>()
                if (name != null) targets.add(ResourcePatcher.Target(R.string.app_name) { name })
                if (icon != null) {
                    // Only the adaptive icon's foreground layer. Pointing the mipmap itself at the
                    // picture turns the icon into a legacy bitmap, which launchers draw with their
                    // own softening — keeping it adaptive is what keeps it crisp.
                    targets.add(
                        ResourcePatcher.Target(R.drawable.ksu_launcher_icon, ResourcePatcher::iconPathLike),
                    )
                }
                val patched = ResourcePatcher.patchInPlace(table, targets) ?: return false
                table = patched.table
                // The picture has to exist under the name the icon entries now point at.
                for (path in patched.files) zip.add(path, ByteArrayInputStream(icon!!), true)
            }
            replacements[RESOURCE_TABLE] = table

            for (abi in ABIS) {
            val name = "lib/$abi/libksud.so"
                val entry = zip.get(name)?.read() ?: continue
                replacements[name] = patchKsudo(entry, from, to)
            }

            // resources.arsc has to stay uncompressed for Android 11+ to install the APK.
            replacements.forEach { (name, bytes) ->
                zip.add(name, ByteArrayInputStream(bytes), name != RESOURCE_TABLE)
            }
            zip.update()
        }

        sign(context, work, target)
        work.delete()
        true
    } catch (e: Exception) {
        Log.w(TAG, "hide: repacking failed", e)
        // Leave the reason where it can be read without racing the log buffer.
        runCatching { File(context.filesDir, "hide-error.txt").writeText(e.toString() + "\n" + e.stackTraceToString()) }
        false
    }

    /** Re-signs [apk] with the key the kernel knows, writing the result to [target]. */
    private fun sign(context: Context, apk: File, target: File) {
        context.assets.open(KEYSTORE_ASSET).use { stream ->
            // Android 13's AOSP BouncyCastle fails to derive the PBES2 MAC key this keystore
            // uses (JDK 21 writes PBES2+PBKDF2+AES_256), so KeyStore.load throws
            // "No installed provider supports this key: PKCS12Key". Register both the AOSP
            // BC (com.android.org.bouncycastle) and, if present, the bundled BC under the
            // org.bouncycastle name, so a SecretKeyFactory for PKCS12/PBES2 is resolvable.
            listOf(
                "com.android.org.bouncycastle.jce.provider.BouncyCastleProvider",
                "org.bouncycastle.jce.provider.BouncyCastleProvider",
            ).forEach { cn ->
                runCatching {
                    val provider = Class.forName(cn).getDeclaredConstructor().newInstance() as java.security.Provider
                    if (Security.getProvider(provider.name) == null) Security.addProvider(provider)
                    Log.i(TAG, "registered keystore provider: ${provider.name}")
                }.onFailure { Log.w(TAG, "cannot register keystore provider $cn: $it") }
            }
            val keystore = KeyStore.getInstance("PKCS12")
                .apply { load(stream, BuildConfig.KSU_KEYSTORE_PASSWORD.toCharArray()) }
            val key = keystore.getKey(BuildConfig.KSU_KEY_ALIAS, BuildConfig.KSU_KEY_PASSWORD.toCharArray()) as PrivateKey
            val chain = keystore.getCertificateChain(BuildConfig.KSU_KEY_ALIAS).map { it as X509Certificate }
            ApkSigner.Builder(listOf(ApkSigner.SignerConfig.Builder(BuildConfig.KSU_KEY_ALIAS, key, chain).build()))
                .setInputApk(apk)
                .setOutputApk(target)
                // minSdk is 31, so v1 has nothing to say. v2 is the block the kernel reads
                // (`manager/apk_sign.c`), and asking for v3 as well makes apksig write *only* a v3
                // block — which the kernel does not look at, so the renamed app would come up as
                // "not installed". Keep this to v2, exactly the scheme the APK was signed with.
                .setV1SigningEnabled(false)
                .setV2SigningEnabled(true)
                .setV3SigningEnabled(false)
                .setMinSdkVersion(Build.VERSION_CODES.S)
                .build()
                .sign()
        }
    }

    /** Swaps the package name in the binary manifest's string pool. */
    private fun patchPackageName(data: ByteArray, from: String, to: String): ByteArray? {
        var patched = data
        for (suffix in PACKAGE_NAME_SUFFIXES) {
            val old = (from + suffix).toByteArray(StandardCharsets.UTF_16LE)
            val at = indexOfPooledString(patched, old) ?: return null
            patched = patched.copyOf().also {
                System.arraycopy((to + suffix).toByteArray(StandardCharsets.UTF_16LE), 0, it, at, old.size)
            }
        }
        return patched
    }

    /**
     * resources.arsc names its package in a fixed-width field of the package chunk rather than in
     * a string pool, so what follows the name there is NUL padding, not the next entry.
     */
    private fun patchResourceTable(data: ByteArray, from: String, to: String): ByteArray? {
        val old = from.toByteArray(StandardCharsets.UTF_16LE)
        val at = indexOfPaddedString(data, old) ?: return null
        return data.copyOf().also {
            System.arraycopy(to.toByteArray(StandardCharsets.UTF_16LE), 0, it, at, old.size)
        }
    }

    /** ksud carries its default package name as a plain Rust string, every time it names it. */
    private fun patchKsudo(data: ByteArray, from: String, to: String): ByteArray {
        val old = from.toByteArray(StandardCharsets.UTF_8)
        val new = to.toByteArray(StandardCharsets.UTF_8)
        val patched = data.copyOf()
        var at = indexOf(patched, old)
        while (at >= 0) {
            System.arraycopy(new, 0, patched, at, old.size)
            at = indexOf(patched, old, at + 1)
        }
        return patched
    }

    /**
     * Finds the one string pool entry that reads exactly [entry].
     *
     * A pool entry is a UTF-16 length, the text, then a UTF-16 NUL — and that NUL is what tells the
     * whole entry apart from the same text sitting inside a longer one, which matters because every
     * derived name starts with the package name. Anything but exactly one match is a no.
     */
    private fun indexOfPooledString(data: ByteArray, entry: ByteArray): Int? {
        val length = entry.size / 2
        var found = -1
        var at = indexOf(data, entry)
        while (at >= 0) {
            val end = at + entry.size
            val terminated = end + 1 < data.size && data[end] == 0.toByte() && data[end + 1] == 0.toByte()
            val precededByLength = at >= 2 && data[at - 2] == length.toByte() && data[at - 1] == 0.toByte()
            if (terminated && precededByLength) {
                if (found >= 0) return null
                found = at
            }
            at = indexOf(data, entry, at + 1)
        }
        return if (found >= 0) found else null
    }

    /** The one occurrence of [entry] that the NUL padding of a fixed-width field follows. */
    private fun indexOfPaddedString(data: ByteArray, entry: ByteArray): Int? {
        var found = -1
        var at = indexOf(data, entry)
        while (at >= 0) {
            val end = at + entry.size
            val padded = end + 3 < data.size && (end..end + 3).all { data[it] == 0.toByte() }
            if (padded) {
                if (found >= 0) return null
                found = at
            }
            at = indexOf(data, entry, at + 1)
        }
        return if (found >= 0) found else null
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
