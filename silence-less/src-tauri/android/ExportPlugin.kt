package fr.silenceless

import android.app.Activity
import android.content.ContentValues
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.OpenableColumns
import androidx.activity.result.ActivityResult
import app.tauri.Logger
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream

@InvokeArg
class ImportArgs {
  lateinit var uris: Array<String>
}

@InvokeArg
class SaveArgs {
  var folderUri: String? = null
}

/**
 * Pont Android pour SilenceLess :
 *  - `importFiles` : copie des fichiers choisis (URI `content://`) dans le
 *    stockage privé de l'app, en renvoyant de vrais chemins lisibles par Rust.
 *  - `saveToDownloads` : exporte les MP3 générés vers Téléchargements/SilenceLess.
 */
@TauriPlugin
class ExportPlugin(private val activity: Activity) : Plugin(activity) {

  private val importDir: File
    get() = File(activity.cacheDir, "import")

  private val outputDir: File
    get() = File(importDir, "mp3_output")

  @Command
  fun importFiles(invoke: Invoke) {
    Thread {
      try {
        val args = invoke.parseArgs(ImportArgs::class.java)
        val dir = importDir
        if (dir.exists()) dir.deleteRecursively()
        dir.mkdirs()

        val paths = mutableListOf<String>()
        for (raw in args.uris) {
          paths.add(importOne(Uri.parse(raw), dir))
        }

        val ret = JSObject()
        ret.put("paths", JSArray.from(paths.toTypedArray()))
        activity.runOnUiThread { invoke.resolve(ret) }
      } catch (e: Exception) {
        val msg = e.message ?: "import failed"
        Logger.error(msg, e)
        activity.runOnUiThread { invoke.reject(msg) }
      }
    }.start()
  }

  @Command
  fun pickOutputFolder(invoke: Invoke) {
    try {
      val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
      intent.addFlags(
        Intent.FLAG_GRANT_READ_URI_PERMISSION or
          Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
          Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION
      )
      startActivityForResult(invoke, intent, "outputFolderResult")
    } catch (e: Exception) {
      invoke.reject(e.message ?: "pick folder failed")
    }
  }

  @ActivityCallback
  fun outputFolderResult(invoke: Invoke, result: ActivityResult) {
    if (result.resultCode == Activity.RESULT_OK) {
      val uri = result.data?.data
      if (uri != null) {
        try {
          activity.contentResolver.takePersistableUriPermission(
            uri,
            Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
          )
        } catch (e: Exception) {
          Logger.error("takePersistableUriPermission failed", e)
        }
        val ret = JSObject()
        ret.put("uri", uri.toString())
        invoke.resolve(ret)
      } else {
        invoke.reject("aucun dossier sélectionné")
      }
    } else {
      invoke.reject("sélection annulée")
    }
  }

  @Command
  fun saveToFolder(invoke: Invoke) {
    Thread {
      try {
        val args = invoke.parseArgs(SaveArgs::class.java)
        val mp3s = outputDir.listFiles { f ->
          f.isFile && f.extension.equals("mp3", ignoreCase = true)
        } ?: emptyArray()

        val saved = mutableListOf<String>()
        val folderUri = args.folderUri?.let { Uri.parse(it) }
        for (f in mp3s) {
          if (folderUri != null) {
            writeToFolder(folderUri, f)
          } else {
            saveToDownloads(f)
          }
          saved.add(f.name)
        }

        val ret = JSObject()
        ret.put("count", saved.size)
        ret.put("names", JSArray.from(saved.toTypedArray()))
        activity.runOnUiThread { invoke.resolve(ret) }
      } catch (e: Exception) {
        val msg = e.message ?: "export failed"
        Logger.error(msg, e)
        activity.runOnUiThread { invoke.reject(msg) }
      }
    }.start()
  }

  private fun importOne(uri: Uri, dir: File): String {
    val name = displayName(uri)
    val dest = File(dir, name)
    activity.contentResolver.openInputStream(uri)?.use { input ->
      FileOutputStream(dest).use { output -> input.copyTo(output) }
    } ?: throw IllegalStateException("impossible d'ouvrir $uri")
    return dest.absolutePath
  }

  private fun displayName(uri: Uri): String {
    var name: String? = null
    val cursor = activity.contentResolver.query(
      uri,
      arrayOf(OpenableColumns.DISPLAY_NAME),
      null,
      null,
      null
    )
    cursor?.use {
      if (it.moveToFirst()) {
        val idx = it.getColumnIndex(OpenableColumns.DISPLAY_NAME)
        if (idx >= 0) name = it.getString(idx)
      }
    }
    if (name.isNullOrBlank()) name = "audio_${System.currentTimeMillis()}.wav"
    return name!!
  }

  private fun writeToFolder(folderUri: Uri, file: File) {
    val resolver = activity.contentResolver
    val treeDocId = DocumentsContract.getTreeDocumentId(folderUri)
    val childrenUri = DocumentsContract.buildChildDocumentsUriUsingTree(folderUri, treeDocId)

    var target: Uri? = null
    resolver.query(
      childrenUri,
      arrayOf(
        DocumentsContract.Document.COLUMN_DOCUMENT_ID,
        DocumentsContract.Document.COLUMN_DISPLAY_NAME
      ),
      null,
      null,
      null
    )?.use { c ->
      while (c.moveToNext()) {
        val docId = c.getString(0)
        val name = c.getString(1)
        if (name == file.name) {
          target = DocumentsContract.buildDocumentUriUsingTree(folderUri, docId)
          break
        }
      }
    }

    val docUri = target
      ?: DocumentsContract.createDocument(resolver, folderUri, "audio/mpeg", file.name)
      ?: throw IllegalStateException("création du document impossible")

    resolver.openOutputStream(docUri)?.use { out ->
      FileInputStream(file).use { it.copyTo(out) }
    } ?: throw IllegalStateException("ouverture en écriture impossible")
  }

  private fun saveToDownloads(file: File) {
    val resolver = activity.contentResolver
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
      val values = ContentValues().apply {
        put(MediaStore.MediaColumns.DISPLAY_NAME, file.name)
        put(MediaStore.MediaColumns.MIME_TYPE, "audio/mpeg")
        put(
          MediaStore.MediaColumns.RELATIVE_PATH,
          "${Environment.DIRECTORY_DOWNLOADS}/SilenceLess"
        )
      }
      val uri = resolver.insert(
        MediaStore.Downloads.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY),
        values
      ) ?: throw IllegalStateException("insertion MediaStore impossible")

      resolver.openOutputStream(uri)?.use { out ->
        FileInputStream(file).use { it.copyTo(out) }
      } ?: throw IllegalStateException("ouverture en écriture impossible")
    } else {
      val dir = File(
        Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS),
        "SilenceLess"
      )
      dir.mkdirs()
      file.copyTo(File(dir, file.name), overwrite = true)
    }
  }
}
