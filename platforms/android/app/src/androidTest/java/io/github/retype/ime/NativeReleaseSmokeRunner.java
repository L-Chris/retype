package io.github.retype.ime;

import android.app.Activity;
import android.app.Instrumentation;
import android.os.Bundle;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import org.json.JSONArray;
import org.json.JSONObject;

/** Exercises the production-shrunk JNI ABI without retaining app APIs for AndroidX Test. */
public final class NativeReleaseSmokeRunner extends Instrumentation {
  private Bundle arguments;

  @Override public void onCreate(Bundle arguments) {
    super.onCreate(arguments);
    this.arguments = arguments;
    start();
  }

  @Override public void onStart() {
    Bundle result = new Bundle();
    try {
      File bundle = new File(getTargetContext().getCacheDir(), "release-resources.tmp");
      try (InputStream input=getContext().getAssets().open("retype-base.retype-resources");
           FileOutputStream output=new FileOutputStream(bundle)) {
        byte[] buffer=new byte[65536];int count;
        while ((count=input.read(buffer))>=0) output.write(buffer,0,count);
      }
      String response=NativeBridge.feature(new JSONObject().put("type","resourceImport")
          .put("root",new File(getTargetContext().getFilesDir(),"dictionaries").getPath()).put("source",bundle.getPath()).toString());
      File dictionary=new File(new JSONObject(response).getString("base"));
      bundle.delete();
      for (boolean flypy : new boolean[] {false, true}) runScheme(dictionary, flypy);
      result.putString("stream", "\nOptimized release JNI: both schemes, preview, full pages, stale generations, late selection and ACK passed.\n");
      finish(Activity.RESULT_OK, result);
    } catch (Throwable error) {
      result.putString("stream", "\nRelease JNI smoke failed: " + error + "\n");
      finish(Activity.RESULT_CANCELED, result);
    }
  }

  private static void check(boolean valid, String message) {
    if (!valid) throw new AssertionError(message);
  }

  private NativeUpdate key(long handle, String key) {
    return NativeBridge.dispatchInput(handle, 1, key, 0, 0, 0);
  }

  private void runScheme(File dictionary, boolean flypy) throws Exception {
    String database = new File(getTargetContext().getCacheDir(), "release-smoke.db").getPath();
    long typed = NativeBridge.create(dictionary.getPath(), database, flypy, true, false, "[]");
    long legacy = NativeBridge.create(dictionary.getPath(), database, flypy, true, false, "[]");
    try {
      NativeUpdate first = key(typed, "n");
      NativeUpdate second = key(typed, "i");
      check(second.getCandidateTotal() > 16 && second.getCandidates().length == 8,
          "preview bound: count=" + second.getCandidateTotal());
      check(NativeBridge.candidates(typed, first.getGeneration(), 0, 64) == null, "stale page");
      List<String> all = new ArrayList<>();
      for (int start = 0; start < second.getCandidateTotal(); start += 64) {
        all.addAll(Arrays.asList(NativeBridge.candidates(typed, second.getGeneration(), start, 64)));
      }
      check(all.size() == second.getCandidateTotal(), "full paging");
      NativeUpdate stale = NativeBridge.dispatchInput(typed, 3, "", 16, first.getGeneration(), 0);
      check(stale.getCommits().length == 0, "stale choice");
      NativeUpdate chosen = NativeBridge.dispatchInput(typed, 3, "", 16, second.getGeneration(), 0);
      check(chosen.getCommits().length == 1 && chosen.getCommits()[0].equals(all.get(16)), "absolute choice");
      NativeBridge.acknowledge(typed, chosen.getGeneration(), true);
      NativeBridge.destroy(typed);
      typed = NativeBridge.create(dictionary.getPath(), database, flypy, true, false, "[]");

      String[] keys = flypy
          ? new String[] {"w", "o", "u", "i", "backspace", "backspace", "backspace", "backspace"}
          : new String[] {"n", "i", "h", "a", "o", "backspace", "backspace", "backspace", "backspace", "backspace"};
      boolean benchmark = arguments != null && "true".equals(arguments.getString("retypeBenchmark"));
      List<Long> typedTimes = new ArrayList<>();
      List<Long> jsonTimes = new ArrayList<>();
      for (int cycle = 0; cycle < (benchmark ? 100 : 2); cycle++) {
        for (String value : keys) {
          long[] times = new long[2];
          int[] totals = new int[2];
          for (int turn = 0; turn < 2; turn++) {
            int path = (cycle + turn) % 2;
            long started = System.nanoTime();
            if (path == 0) {
              NativeUpdate update = key(typed, value);
              totals[0] = update.getCandidateTotal();
              NativeBridge.acknowledge(typed, update.getGeneration(), false);
            } else {
              JSONObject update = new JSONObject(NativeBridge.dispatch(legacy,
                  new JSONObject().put("type", "key").put("value", value).toString()));
              JSONArray candidates = update.getJSONArray("candidates");
              List<String> words = new ArrayList<>();
              for (int i = 0; i < candidates.length(); i++) words.add(candidates.getString(i));
              totals[1] = words.size();
              NativeBridge.dispatch(legacy, new JSONObject().put("type", "ack")
                  .put("generation", update.getLong("generation")).put("accepted", false).toString());
            }
            times[path] = System.nanoTime() - started;
          }
          check(totals[0] == totals[1], "candidate total parity");
          if (benchmark && cycle >= 20) { typedTimes.add(times[0]); jsonTimes.add(times[1]); }
        }
      }
      if (benchmark) {
        Bundle status = new Bundle();
        status.putString("stream", "\ntransport scheme=" + (flypy ? "flypy" : "full")
            + " samples=" + typedTimes.size() + " typed:" + percentiles(typedTimes)
            + " json:" + percentiles(jsonTimes) + "\n");
        sendStatus(0, status);
      }
    } finally { NativeBridge.destroy(typed); NativeBridge.destroy(legacy); }
  }

  private static String percentiles(List<Long> values) {
    long[] sorted = new long[values.size()];
    for (int i = 0; i < sorted.length; i++) sorted[i] = values.get(i);
    Arrays.sort(sorted);
    return "p50_us=" + sorted[(sorted.length - 1) / 2] / 1000
        + ",p95_us=" + sorted[(sorted.length - 1) * 95 / 100] / 1000
        + ",p99_us=" + sorted[(sorted.length - 1) * 99 / 100] / 1000;
  }
}
