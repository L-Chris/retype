#ifndef RETYPE_MACOS_H
#define RETYPE_MACOS_H
#include <stdbool.h>
#include <stdint.h>
#include <stddef.h>
typedef struct RtRuntime RtRuntime;
typedef struct RtSession RtSession;
typedef struct RtResources RtResources;
// All returned strings are owned UTF-8 JSON; release them with rt_string_free.
// Session calls are serialized. Open runtimes and run resource operations off the UI thread.
char *rt_runtime_open(const char *root, RtRuntime **out);
void rt_runtime_free(RtRuntime *runtime);
RtSession *rt_session_new(const RtRuntime *runtime, bool flypy, bool chinese);
void rt_session_free(RtSession *session);
char *rt_session_send(RtSession *session, const char *command);
void rt_string_free(char *text);
// Feature calls may perform network or file IO and must run on a worker.
char *rt_feature(const char *command);
char *rt_runtime_reload(const RtRuntime *runtime, const char *root, uint32_t pack_mask);
char *rt_runtime_voice(const RtRuntime *runtime, const char *text);
// Returns {"saved":true} after the learning/statistics barrier, or {"error":...}.
char *rt_runtime_flush(const RtRuntime *runtime);
void rt_macos_hooks(intptr_t (*read)(const char *,uint8_t *,size_t),bool (*save)(const char *,const char *),intptr_t (*clipboard_read)(uint8_t *,size_t,uint64_t *),bool (*clipboard_write)(const char *,uint64_t,uint64_t *));
RtResources *rt_resources_new(const char *root);
void rt_resources_begin(const RtResources *resources);
char *rt_resources_run(const RtResources *resources, int32_t action, const char *source);
void rt_resources_cancel(const RtResources *resources);
uint32_t rt_resources_progress(const RtResources *resources);
void rt_resources_free(RtResources *resources);
#endif
