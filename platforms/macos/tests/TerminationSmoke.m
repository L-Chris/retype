#import "App.h"
#include <stdio.h>
#include <unistd.h>

extern NSDictionary *(^RTTestRuntimeFlush)(void);
static BOOL flushed;
static BOOL cancelled;
@interface RTAppDelegate (TerminationTest)
- (void)finishTermination:(NSDictionary *)result;
@end
@interface RTTerminationDelegate : RTAppDelegate
@property(nonatomic,strong) NSTask *testInstaller;
@end
@implementation RTTerminationDelegate
- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    (void)notification;
    self.messages=NSMutableDictionary.dictionary;self.jobs=NSMutableSet.set;
    BOOL fail=[NSProcessInfo.processInfo.arguments containsObject:@"--fail-flush"];
    if(fail){[self.jobs addObject:@"updates"];self.testInstaller=[NSTask new];self.testInstaller.executableURL=[NSURL fileURLWithPath:@"/bin/sleep"];self.testInstaller.arguments=@[@"60"];if(![self.testInstaller launchAndReturnError:NULL])_exit(6);self.updateInstaller=self.testInstaller;}
    __block BOOL failed=NO;
    RTTestRuntimeFlush=^NSDictionary *{
        if(NSThread.isMainThread)_exit(3);
        if(fail&&!failed){failed=YES;return RTJSON(strdup("{\"error\":\"synthetic flush failure\"}"));}
        // Use the real Rust barrier and production decoder together. A synthetic
        // NSDictionary hid the scalar success response that broke real updates.
        NSDictionary *result=RTJSON(rt_runtime_flush(self.runtime));flushed=YES;return result;
    };
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY,0),^{
        RtRuntime *runtime=NULL;NSDictionary *opened=RTJSON(rt_runtime_open([RTDataRoot() stringByAppendingPathComponent:@"termination"].UTF8String,&runtime));
        if(opened[@"error"]||!runtime)_exit(8);
        // Reproduce termination from an update completion on the main dispatch queue.
        dispatch_async(dispatch_get_main_queue(),^{self.runtime=runtime;[NSApp terminate:nil];});
    });
}
- (void)voiceCancel {}
- (void)finishTermination:(NSDictionary *)result {
    [super finishTermination:result];
    if(result[@"error"])[self performSelector:@selector(checkCancellation:) withObject:nil afterDelay:.05];
}
- (void)checkCancellation:(id)sender {
    (void)sender;
    if(self.testInstaller.running||self.updateInstaller||[self.jobs containsObject:@"updates"])_exit(7);
    cancelled=YES;
    puts("Failed saves cancel termination, stop the pending installer and allow retry.");fflush(stdout);
    dispatch_async(dispatch_get_main_queue(),^{[NSApp terminate:nil];});
}
- (void)applicationWillTerminate:(NSNotification *)notification {
    (void)notification;
    if([NSProcessInfo.processInfo.arguments containsObject:@"--fail-flush"]&&!cancelled)_exit(5);
    if(!flushed)_exit(4);
    puts("Update termination from the main queue drains learning/statistics and exits without a nested-loop deadlock.");
}
@end
int main(void){@autoreleasepool{
    if(!NSProcessInfo.processInfo.environment[@"RETYPE_TEST_ROOT"].length)return 1;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW,3*NSEC_PER_SEC),dispatch_get_global_queue(QOS_CLASS_UTILITY,0),^{fputs("Termination watchdog expired.\n",stderr);_exit(2);});
    NSApplication *app=NSApplication.sharedApplication;
    __attribute__((objc_precise_lifetime)) RTTerminationDelegate *delegate=[RTTerminationDelegate new];app.delegate=delegate;
    [app run];return 0;
}}
