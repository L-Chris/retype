package io.github.retype.ime

import org.json.JSONArray
import org.json.JSONObject

internal object AndroidPackages {
  fun names(version: String, supportedAbis: List<String>): List<String> {
    val result = JSONObject(NativeBridge.feature(JSONObject().put("type", "updatePackageNames")
        .put("version", version).put("abis", JSONArray(supportedAbis)).toString())).getJSONArray("names")
    return List(result.length()) { result.getString(it) }
  }
}
