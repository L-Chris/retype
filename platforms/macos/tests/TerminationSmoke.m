#import "App.h"
#include <stdio.h>
#include <unistd.h>

extern NSDictionary *(^RTTestRuntimeFlush)(void);
static BOOL flushed;
@interface RTAppDelegate (TerminationTest)
- (void)finishTermination:(NSDictionary *)result;
@end
@interface RTTerminationDelegate : RTAppDelegate
@property(nonatomic,strong) NSTask *testInstaller;
@end
@implementation RTTerminationDelegate
- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    (void)notification;
    self.runtime=(RtRuntime *)1; // The flush hook below owns this test's synthetic runtime.
    self.messages=NSMutableDictionary.dictionary;self.jobs=NSMutableSet.set;
    BOOL fail=[NSProcessInfo.processInfo.arguments containsObject:@"--fail-flush"];
    if(fail){[self.jobs addObject:@"updates"];self.testInstaller=[NSTask new];self.testInstaller.executableURL=[NSURL fileURLWithPath:@"/bin/sleep"];self.testInstaller.arguments=@[@"60"];if(![self.testInstaller launchAndReturnError:NULL])_exit(6);self.updateInstaller=self.testInstaller;}
    RTTestRuntimeFlush=^NSDictionary *{
        if(NSThread.isMainThread)_exit(3);
        usleep(20000);flushed=YES;return fail?@{@"error":@"synthetic flush failure"}:@{@"value":@YES};
    };
    // Reproduce termination from an update completion on the main dispatch queue.
    dispatch_async(dispatch_get_main_queue(),^{[NSApp terminate:nil];});
}
- (void)voiceCancel {}
- (void)finishTermination:(NSDictionary *)result {
    [super finishTermination:result];
    if(result[@"error"])[self performSelector:@selector(checkCancellation:) withObject:nil afterDelay:.05];
}
- (void)checkCancellation:(id)sender {
    (void)sender;
    if(self.testInstaller.running||self.updateInstaller||[self.jobs containsObject:@"updates"])_exit(7);
    puts("Failed saves cancel termination, stop the pending installer and allow retry.");fflush(stdout);_exit(0);
}
- (void)applicationWillTerminate:(NSNotification *)notification {
    (void)notification;
    if([NSProcessInfo.processInfo.arguments containsObject:@"--fail-flush"])_exit(5);
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
