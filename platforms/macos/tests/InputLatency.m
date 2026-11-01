#import "App.h"
#import <QuartzCore/QuartzCore.h>
#include <stdio.h>
#include <stdlib.h>
#include <pthread.h>

static void Check(BOOL ok,NSString *message){if(!ok){fprintf(stderr,"Input latency: %s\n",message.UTF8String);exit(1);}}
@interface RTLatencyTextView:NSTextView
@end
@implementation RTLatencyTextView
// IMK's client proxy uses selectionRange; NSTextInputClient uses selectedRange.
- (void)setMarkedText:(id)value selectionRange:(NSRange)selection replacementRange:(NSRange)replacement {
    [super setMarkedText:value selectedRange:selection replacementRange:replacement];
}
// Drive only the isolated controller, without activating the user's selected IME.
- (NSTextInputContext *)inputContext { return nil; }
- (NSDictionary *)attributesForCharacterIndex:(NSUInteger)index lineHeightRectangle:(NSRect *)rect {
    (void)index;*rect=[self firstRectForCharacterRange:NSMakeRange(self.selectedRange.location,0) actualRange:NULL];return @{};
}
- (NSInteger)windowLevel {return self.window.level;}
@end
@interface RetypeInputController (Latency)
- (BOOL)ensureSession;
@end
static NSEvent *Event(NSString *value,NSWindow *window) {
    return [NSEvent keyEventWithType:NSEventTypeKeyDown location:NSZeroPoint modifierFlags:0
        timestamp:NSProcessInfo.processInfo.systemUptime windowNumber:window.windowNumber context:nil
        characters:value charactersIgnoringModifiers:value isARepeat:NO keyCode:[value isEqual:@"\177"]?51:0];
}
static NSDictionary *Percentiles(NSArray<NSDictionary *> *rows,NSString *key) {
    NSArray *values=[[rows valueForKey:key] sortedArrayUsingSelector:@selector(compare:)];
    if(!values.count)return @{};
    return @{@"p50":values[values.count/2],@"p95":values[(NSUInteger)ceil(values.count*.95)-1],
        @"p99":values[(NSUInteger)ceil(values.count*.99)-1],@"maximum":values.lastObject};
}
static void Screenshot(NSView *view,NSString *path) {
    NSBitmapImageRep *bitmap=[view bitmapImageRepForCachingDisplayInRect:view.bounds];
    [view cacheDisplayInRect:view.bounds toBitmapImageRep:bitmap];
    Check([[bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}] writeToFile:path atomically:YES],@"Screenshot saved");
}
int main(int argc,const char *argv[]){@autoreleasepool{
    Check(pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE,0)==0,@"Interactive main-thread QoS");
    id activity=[NSProcessInfo.processInfo beginActivityWithOptions:NSActivityUserInitiated|NSActivityLatencyCritical
        reason:@"Measure active native input without App Nap"];
    Check(argc>=3,@"Usage: input-latency expected-first output-directory [rounds]");
    NSString *expected=[NSString stringWithUTF8String:argv[1]],*output=[NSString stringWithUTF8String:argv[2]];
    NSUInteger rounds=argc>3?MAX(1,atoi(argv[3])):40;
    Check(NSProcessInfo.processInfo.environment[@"RETYPE_TEST_ROOT"].length>0,@"Private test root required");
    [NSFileManager.defaultManager createDirectoryAtPath:output withIntermediateDirectories:YES attributes:nil error:NULL];
    NSApplication *app=NSApplication.sharedApplication;[app setActivationPolicy:NSApplicationActivationPolicyRegular];
    __attribute__((objc_precise_lifetime)) RTAppDelegate *delegate=[RTAppDelegate new];app.delegate=delegate;
    delegate.messages=NSMutableDictionary.dictionary;delegate.jobs=NSMutableSet.set;
    __block RtRuntime *runtime=NULL;__block NSDictionary *opened;
    dispatch_sync(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED,0),^{opened=RTJSON(rt_runtime_open(RTDataRoot().UTF8String,&runtime));});
    Check(runtime&&!opened[@"error"],opened[@"error"]?:@"Runtime opens");delegate.runtime=runtime;
    RTPreferences.shared.value[@"input"][@"scheme"]=@YES;
    RTPreferences.shared.value[@"input"][@"english"]=@NO;
    RTPreferences.shared.value[@"input"][@"symbols"]=@NO;
    NSWindow *window=[[NSWindow alloc] initWithContentRect:NSMakeRect(120,340,720,220)
        styleMask:NSWindowStyleMaskTitled|NSWindowStyleMaskClosable backing:NSBackingStoreBuffered defer:NO];
    window.title=@"retype native input latency validation";window.releasedWhenClosed=NO;
    RTLatencyTextView *editor=[[RTLatencyTextView alloc] initWithFrame:window.contentView.bounds];
    editor.font=[NSFont systemFontOfSize:20];editor.textContainerInset=NSMakeSize(20,30);window.contentView=editor;
    [app finishLaunching];[window makeKeyAndOrderFront:nil];[window makeFirstResponder:editor];[app activateIgnoringOtherApps:YES];
    RetypeInputController *controller=[RetypeInputController new];controller.active=YES;controller.chinese=YES;
    controller.activeClient=(id<IMKTextInput>)editor;controller.closers=NSMutableArray.array;delegate.editor=controller;
    Check([controller ensureSession],@"Real input session starts");
    NSMutableArray *reports=NSMutableArray.array;
    for(NSString *input in @[@"hduiyzdmm",@"wdsssssssssss",@"woui"]){
        NSMutableArray *warm=NSMutableArray.array,*cold=NSMutableArray.array;
        for(NSUInteger round=0;round<=rounds;round++){
            NSMutableArray *events=NSMutableArray.array;
            for(NSUInteger i=0;i<input.length;i++)[events addObject:[input substringWithRange:NSMakeRange(i,1)]];
            for(NSUInteger i=0;i<input.length;i++)[events addObject:@"\177"];
            for(NSUInteger step=0;step<events.count;step++){@autoreleasepool{
                double commands=RTTestCommandSeconds,draw=RTTestCandidateDrawSeconds,widths=RTTestCandidateWidthSeconds;
                CFAbsoluteTime started=CFAbsoluteTimeGetCurrent();
                Check([controller handleEvent:Event(events[step],window) client:editor],@"Input controller handles keystroke");
                double handled=(CFAbsoluteTimeGetCurrent()-started)*1000;
                [window displayIfNeeded];[[RTCandidatePanel shared] displayIfNeeded];[CATransaction flush];
                double total=(CFAbsoluteTimeGetCurrent()-started)*1000;
                NSDictionary *row=@{@"direction":step<input.length?@"type":@"backspace",
                    @"remaining":@(step<input.length?step+1:events.count-step-1),
                    @"handler_ms":@(handled),@"commands_ms":@((RTTestCommandSeconds-commands)*1000),
                    @"widths_ms":@((RTTestCandidateWidthSeconds-widths)*1000),
                    @"candidate_draw_ms":@((RTTestCandidateDrawSeconds-draw)*1000),@"submit_frame_ms":@(total)};
                [(round?warm:cold) addObject:row];
                if([input isEqual:@"hduiyzdmm"]&&step==input.length-1){
                    Check([controller.state[@"candidates"][0] isEqual:expected],@"First candidate matches the expected build");
                    if(round==rounds){Screenshot(editor,[output stringByAppendingPathComponent:@"editor.png"]);Screenshot(RTCandidatePanel.shared.contentView,[output stringByAppendingPathComponent:@"candidates.png"]);}
                }
                [NSRunLoop.mainRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:.001]];
            }}
            Check(!editor.string.length,@"Backspace empties the real NSTextView");
        }
        NSMutableDictionary *metrics=NSMutableDictionary.dictionary;
        for(NSString *key in @[@"handler_ms",@"commands_ms",@"widths_ms",@"candidate_draw_ms",@"submit_frame_ms"])metrics[key]=Percentiles(warm,key);
        [reports addObject:@{@"input":input,@"rounds":@(rounds),@"samples":@(warm.count),@"cold":cold,@"metrics":metrics,@"warm":warm}];
    }
    NSData *json=[NSJSONSerialization dataWithJSONObject:reports options:NSJSONWritingPrettyPrinted error:NULL];
    Check([json writeToFile:[output stringByAppendingPathComponent:@"timings.json"] atomically:YES],@"Timing report saved");
    printf("%s\n",[[[NSString alloc] initWithData:json encoding:NSUTF8StringEncoding] UTF8String]);
    [controller.timer invalidate];controller.timer=nil;controller.active=NO;[RTCandidatePanel.shared hideFor:controller];
    rt_session_free(controller.session);controller.session=NULL;delegate.runtime=NULL;
    dispatch_sync(dispatch_get_global_queue(QOS_CLASS_UTILITY,0),^{RTJSON(rt_runtime_flush(runtime));rt_runtime_free(runtime);});
    [window close];[NSProcessInfo.processInfo endActivity:activity];return 0;
}}
