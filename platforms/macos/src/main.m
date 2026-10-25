#import "App.h"
#import <Carbon/Carbon.h>

NSDictionary *RTJSON(char *owned) {
    if (!owned) return @{ @"error": @"输入内核没有响应" };
    NSData *data = [[NSString stringWithUTF8String:owned] dataUsingEncoding:NSUTF8StringEncoding];
    rt_string_free(owned);
    id value = data ? [NSJSONSerialization JSONObjectWithData:data options:NSJSONReadingMutableContainers error:NULL] : nil;
    return [value isKindOfClass:NSDictionary.class] ? value : @{ @"error": @"输入内核响应无效" };
}
NSDictionary *RTSend(RtSession *session, NSDictionary *command) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:command options:0 error:NULL];
    NSString *json = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
    return RTJSON(rt_session_send(session, json.UTF8String));
}
NSString *RTDataRoot(void) {
    #ifdef RT_UI_SMOKE
    return NSProcessInfo.processInfo.environment[@"RETYPE_TEST_ROOT"];
    #else
    return [NSHomeDirectory() stringByAppendingPathComponent:@"Library/Application Support/retype"];
    #endif
}

@implementation RTAppDelegate
- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    (void)notification;
    #ifdef RT_UI_SMOKE
    return;
    #else
    self.chinese = YES;
    self.messages = NSMutableDictionary.dictionary;
    self.jobs = NSMutableSet.set;
    RTRegisterHooks();
    (void)RTPreferences.shared;
    self.resources = rt_resources_new(RTDataRoot().UTF8String);
    self.resourceMessage = @"";
    [self loadRuntime];
    [self startMaintenance];
    if ([NSProcessInfo.processInfo.arguments containsObject:@"--settings"]) [self showSettings:@"launch_argument"];
    #endif
}
- (void)applicationWillTerminate:(NSNotification *)notification {
    (void)notification;
    [self.maintenance invalidate];
    [self voiceCancel];
    // Background jobs may still hold these process-lived handles; the OS releases
    // them on exit after the acknowledged learning/statistics barrier below.
}
- (NSApplicationTerminateReply)applicationShouldTerminate:(NSApplication *)sender {
    [self voiceCancel];[self.maintenance invalidate];
    if(!self.runtime)return NSTerminateNow;
    RtRuntime *runtime=self.runtime;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY,0),^{RTJSON(rt_runtime_flush(runtime));dispatch_async(dispatch_get_main_queue(),^{[sender replyToApplicationShouldTerminate:YES];});});
    return NSTerminateLater;
}
- (BOOL)applicationShouldHandleReopen:(NSApplication *)sender hasVisibleWindows:(BOOL)visible {
    (void)sender; (void)visible;
    [self showSettings:@"reopen"];
    return NO;
}
- (void)loadRuntime {
    if (self.loading || self.runtime) return;
    self.loading = YES;
    uint32_t mask=[RTPreferences.shared.value[@"packs"] unsignedIntValue];
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        RtRuntime *runtime = NULL;
        NSDictionary *result = RTJSON(rt_runtime_open(RTDataRoot().UTF8String, &runtime));
        if (runtime) RTJSON(rt_runtime_reload(runtime,RTDataRoot().UTF8String,mask));
        dispatch_async(dispatch_get_main_queue(), ^{
            self.runtime = runtime;
            self.loading = NO;
            [self reloadRuntime];
            if (result[@"error"]) self.resourceMessage = result[@"error"];
            [RTSettings refreshIfLoaded];
        });
    });
}
- (void)showSettings:(id)sender {
    NSString *source=[sender isKindOfClass:NSString.class]?sender:@"application";
    RTSettingsOpenLog(@"requested",source,NO);
    RTSettings *settings = [RTSettings shared];
    [RTSettings refreshIfLoaded];
    settings.window.collectionBehavior |= NSWindowCollectionBehaviorMoveToActiveSpace;
    [NSApp unhide:nil];
    if(settings.window.miniaturized)[settings.window deminiaturize:nil];
    [settings showWindow:nil];
    [NSApp activateIgnoringOtherApps:YES];
    [settings.window makeKeyAndOrderFront:nil];
    RTSettingsOpenLog(@"presented",source,settings.window.visible);
}
- (void)runResources:(int)action source:(NSString *)source {
    if (self.resourceBusy) return;
    self.resourceBusy = YES;
    rt_resources_begin(self.resources);
    self.resourceMessage = action == 1 ? @"正在下载词库…" : @"正在导入词库…";
    [[RTSettings shared] refresh];
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        NSDictionary *result = RTJSON(rt_resources_run(self.resources, action, source.UTF8String));
        dispatch_async(dispatch_get_main_queue(), ^{
            self.resourceBusy = NO;
            self.resourceMessage = result[@"error"] ?: @"词库已安装，可离线输入";
            [self loadRuntime];
            [self reloadRuntime];
            [RTSettings refreshIfLoaded];
        });
    });
}
@end

#ifndef RT_UI_SMOKE
int main(int argc, const char *argv[]) {
    @autoreleasepool {
        // Registration is an explicit installer step, not part of each launch.
        if (argc > 1 && strcmp(argv[1], "--register") == 0) {
            OSStatus status = TISRegisterInputSource((__bridge CFURLRef)NSBundle.mainBundle.bundleURL);
            return status == noErr ? 0 : 1;
        }
        NSApplication *app = NSApplication.sharedApplication;
        __attribute__((objc_precise_lifetime)) RTAppDelegate *delegate = [RTAppDelegate new];
        app.delegate = delegate;
        NSString *connection = NSBundle.mainBundle.infoDictionary[@"InputMethodConnectionName"];
        __attribute__((objc_precise_lifetime)) IMKServer *server = [[IMKServer alloc] initWithName:connection bundleIdentifier:NSBundle.mainBundle.bundleIdentifier];
        if (!server) { NSLog(@"retype: unable to start InputMethodKit server"); return 1; }
        [app run];
        (void)server;
    }
    return 0;
}
#endif
