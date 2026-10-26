#import "App.h"
#include <stdio.h>
#include <stdlib.h>
#include <math.h>

static void Check(BOOL condition,NSString *message){if(!condition){fprintf(stderr,"UI assertion: %s\n",message.UTF8String);exit(1);}}
static NSView *Find(NSView *root,NSString *identifier){if([root.identifier isEqual:identifier])return root;for(NSView *child in root.subviews){NSView *found=Find(child,identifier);if(found)return found;}return nil;}
static void Inspect(NSView *root){
    for(NSView *view in root.subviews){
        if([view isKindOfClass:NSTextField.class]){NSTextField *field=(NSTextField *)view;
            if(field.editable&&field.identifier.length){Check(fabs(NSHeight(field.frame)-34)<1,@"Form fields must have one height");}
            else if(!field.editable)Check(!field.selectable,@"Static labels must not be selectable");
        }
        Inspect(view);
    }
}
@interface RTCandidateTestOwner:NSObject<RTCandidateOwner>@end
@implementation RTCandidateTestOwner
- (void)chooseCandidate:(NSUInteger)index generation:(uint64_t)generation{(void)index;(void)generation;}
@end
extern NSUInteger RTAXFocusQueries;
extern NSString *(^RTTestSecretRead)(NSString *);
extern BOOL (^RTTestSecretSave)(NSString *,NSString *);
@interface RTSettings (UITest)
- (void)navigate:(NSButton *)sender;
- (void)write:(NSString *)path value:(id)value;
@end
static void PumpUntil(BOOL (^done)(void)){
    CFAbsoluteTime deadline=CFAbsoluteTimeGetCurrent()+3;
    while(!done()&&CFAbsoluteTimeGetCurrent()<deadline)[NSRunLoop.mainRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:.01]];
    Check(done(),@"Async settings operation must finish");
}
static void CheckInputMenuSettings(RTSettings *settings){
    RetypeInputController *controller=[RetypeInputController new];
    NSMenuItem *item=[controller menu].itemArray.lastObject;
    Check(item.target==controller&&item.action==@selector(showPreferences:),@"The input menu uses the controller's standard preferences action");
    [settings.window orderOut:nil];
    [controller doCommandBySelector:item.action commandDictionary:@{kIMKCommandMenuItemName:item}];
    PumpUntil(^BOOL{return settings.window.visible;});
    [settings.window miniaturize:nil];
    [controller doCommandBySelector:NSSelectorFromString(@"showSettings:") commandDictionary:@{}];
    PumpUntil(^BOOL{return settings.window.visible&&!settings.window.miniaturized;});
    printf("System input-menu settings commands reopen hidden/minimized windows; legacy selectors remain supported.\n");
}
static void CheckAsyncSettings(RTSettings *settings){
    dispatch_semaphore_t entered=dispatch_semaphore_create(0),release=dispatch_semaphore_create(0);
    RTTestSecretRead=^NSString *(NSString *account){
        (void)account;Check(!NSThread.isMainThread,@"Settings must read Keychain off the main thread");
        dispatch_semaphore_signal(entered);dispatch_semaphore_wait(release,DISPATCH_TIME_FOREVER);return @"synthetic-key";
    };
    [settings setValue:@2 forKey:@"page"];[settings refresh];
    Check(dispatch_semaphore_wait(entered,dispatch_time(DISPATCH_TIME_NOW,2*NSEC_PER_SEC))==0,@"Secret read starts in the background");
    CFAbsoluteTime start=CFAbsoluteTimeGetCurrent();NSButton *target=[NSButton new];target.tag=0;[settings navigate:target];
    Check(CFAbsoluteTimeGetCurrent()-start<.5,@"Switching tabs must finish while Keychain is blocked");
    dispatch_semaphore_signal(release);
    PumpUntil(^BOOL{return [[settings valueForKey:@"secretLoading"] count]==0;});RTTestSecretRead=nil;
    __block NSUInteger writes=0;__block BOOL saved=NO,requestDone=NO,readAfterSave=NO;
    RTTestSecretSave=^BOOL(NSString *account,NSString *value){
        (void)account;(void)value;Check(!NSThread.isMainThread,@"Settings must save Keychain off the main thread");writes++;
        dispatch_semaphore_signal(entered);dispatch_semaphore_wait(release,DISPATCH_TIME_FOREVER);saved=YES;return YES;
    };
    RTTestSecretRead=^NSString *(NSString *account){
        (void)account;Check(!NSThread.isMainThread&&saved,@"Feature credential reads must run after queued settings writes");
        readAfterSave=YES;return @"synthetic-key";
    };
    [settings write:@"secret:ui-test" value:@"synthetic-key"];[settings write:@"secret:ui-test" value:@"synthetic-key"];
    Check(dispatch_semaphore_wait(entered,dispatch_time(DISPATCH_TIME_NOW,2*NSEC_PER_SEC))==0,@"Secret write starts in the background");
    [(RTAppDelegate *)NSApp.delegate feature:@{@"type":@"ui-test-unknown-operation"} secretAccount:@"ui-test" field:@"key" completion:^(NSDictionary *result){
        (void)result;Check(NSThread.isMainThread,@"Feature completion returns to AppKit");requestDone=YES;
    }];
    start=CFAbsoluteTimeGetCurrent();target.tag=1;[settings navigate:target];
    Check(CFAbsoluteTimeGetCurrent()-start<.5,@"Switching tabs must finish while a Keychain save is blocked");
    dispatch_semaphore_signal(release);
    PumpUntil(^BOOL{return requestDone;});
    RTTestSecretSave=nil;RTTestSecretRead=nil;
    Check(writes==1&&readAfterSave,@"An unchanged credential must not be saved twice; actions use the completed save");
    printf("Tab navigation stays responsive during blocked Keychain reads/writes; duplicate saves skipped.\n");
}
@interface RTDeletionClient : NSObject
@property(nonatomic,copy) NSString *text;
@property(nonatomic) NSRange selection;
@property(nonatomic) NSUInteger reads;
@property(nonatomic) BOOL partialRead;
@end
@implementation RTDeletionClient
- (NSUInteger)length {return self.text.length;}
- (NSRange)selectedRange {self.reads++;return self.selection;}
- (NSString *)stringFromRange:(NSRange)range actualRange:(NSRangePointer)actual {
    self.reads++;if(NSMaxRange(range)>self.text.length)return nil;
    *actual=range;if(self.partialRead&&actual->length)actual->length--;return [self.text substringWithRange:range];
}
- (void)insertText:(id)text replacementRange:(NSRange)range {
    self.text=[self.text stringByReplacingCharactersInRange:range withString:text];
}
@end
static void CheckDeletion(void){
    RetypeInputController *controller=[RetypeInputController new];
    RTDeletionClient *client=[RTDeletionClient new];client.text=@"hello";client.selection=NSMakeRange(5,0);
    controller.activeClient=(id<IMKTextInput>)client;controller.closers=NSMutableArray.array;controller.chinese=YES;
    NSUInteger queries=RTAXFocusQueries;
    CFAbsoluteTime start=CFAbsoluteTimeGetCurrent();
    for(NSUInteger i=0;i<1000;i++)Check(![controller pairSymbol:@"backspace"],@"Ordinary deletion belongs to the host");
    Check(client.reads==0&&RTAXFocusQueries==queries,@"Repeated ordinary deletion must not query the client or Accessibility");
    printf("1000 ordinary backspaces: %.2f ms, no client/AX queries.\n",(CFAbsoluteTimeGetCurrent()-start)*1000);
    for(NSString *key in @[@"a",@"1",@" ",@".",@"\177"])Check(![controller pairSymbol:key],@"Non-pairing characters belong to the input engine");
    Check(RTAXFocusQueries==queries,@"Ordinary characters must not query Accessibility");
    client.text=@"a（）b";client.selection=NSMakeRange(2,0);[controller.closers addObject:@"）"];
    Check([controller pairSymbol:@"backspace"]&&[client.text isEqual:@"ab"]&&!controller.closers.count,@"Backspace removes a tracked pair through the input-method client");
    client.text=@"abc";client.selection=NSMakeRange(2,0);[controller.closers addObject:@"）"];
    Check(![controller pairSymbol:@"backspace"]&&[client.text isEqual:@"abc"],@"A stale closer must not delete unrelated text");
    client.text=@"（）";client.selection=NSMakeRange(0,2);
    Check(![controller pairSymbol:@"backspace"],@"Selection deletion belongs to the host");
    Check(RTAXFocusQueries==queries,@"Paired deletion must not query Accessibility either");
}
static void CheckTranslationCapture(void){
    RetypeInputController *controller=[RetypeInputController new];controller.active=YES;
    RTDeletionClient *client=[RTDeletionClient new];controller.activeClient=(id<IMKTextInput>)client;
    NSUInteger queries=RTAXFocusQueries;
    NSString *prefix=[@"x" stringByPaddingToLength:70000 withString:@"x" startingAtIndex:0];
    client.text=[prefix stringByAppendingString:@"Hello 🌎 suffix"];
    client.selection=[client.text rangeOfString:@"Hello 🌎"];
    NSDictionary *capture=[controller captureText];
    Check([capture[@"text"] isEqual:@"Hello 🌎"]&&[capture[@"selected"] boolValue],@"Only the selected UTF-16 range is captured, even in a large document");
    Check([controller replaceCapture:capture text:@"你好 🌎"]&&[client.text isEqual:[prefix stringByAppendingString:@"你好 🌎 suffix"]],@"Translation replaces the selection and preserves surrounding text");
    client.text=@"before Hello after Hello";client.selection=[client.text rangeOfString:@"Hello"];
    capture=[controller captureText];client.selection=[client.text rangeOfString:@"Hello" options:NSBackwardsSearch];
    Check(![controller replaceCapture:capture text:@"你好"],@"Moving the selection to identical text invalidates the capture");
    client.selection=[client.text rangeOfString:@"Hello"];capture=[controller captureText];
    client.text=@"before Hallo after Hello";
    Check(![controller replaceCapture:capture text:@"你好"],@"Changed selected text must not be overwritten");
    client.text=@"Hello";client.selection=NSMakeRange(5,0);capture=[controller captureText];
    Check([capture[@"text"] isEqual:@"Hello"]&&![capture[@"selected"] boolValue],@"An empty selection preserves full-field translation");
    client.partialRead=YES;Check(![controller replaceCapture:capture text:@"你好"],@"Partial text reads must not be replaced");client.partialRead=NO;
    Check([controller replaceCapture:capture text:@"你好"]&&[client.text isEqual:@"你好"],@"An unchanged full field can be replaced");
    client.text=@"Hello";client.selection=NSMakeRange(0,5);capture=[controller captureText];controller.epoch++;
    Check(![controller replaceCapture:capture text:@"你好"],@"An input epoch change invalidates the translation");
    Check(RTAXFocusQueries==queries,@"Supported translation clients must not need an AX fallback");
    printf("Selection translation, Unicode, full-field fallback and stale-capture checks passed.\n");
}
int main(void){@autoreleasepool{
    Check(NSProcessInfo.processInfo.environment[@"RETYPE_TEST_ROOT"].length>0,@"A private test root is required");
    NSApplication *app=NSApplication.sharedApplication;
    __attribute__((objc_precise_lifetime)) RTAppDelegate *delegate=[RTAppDelegate new];app.delegate=delegate;
    delegate.messages=NSMutableDictionary.dictionary;delegate.jobs=NSMutableSet.set;delegate.resourceMessage=@"";
    delegate.resources=rt_resources_new(RTDataRoot().UTF8String);
    NSMutableDictionary *p=RTPreferences.shared.value;
    p[@"ai"][@"providers"]=[@[[ @{@"id":@"validation-provider",@"preset":@"自定义",@"name":@"测试提供商",@"kind":@"Compatible",@"base_url":@"https://example.invalid/v1",@"models":[@[@"test-model"] mutableCopy],@"voice_protocols":NSMutableDictionary.dictionary} mutableCopy]] mutableCopy];
    p[@"ai"][@"provider"]=@"validation-provider";p[@"ai"][@"model"]=@"test-model";
    p[@"ai"][@"voice"][@"provider"]=@"validation-provider";p[@"ai"][@"voice"][@"model"]=@"test-model";
    NSDictionary *count=@{@"chinese":@1066,@"english":@213,@"chineseMs":@1050000,@"englishMs":@47000,@"chineseSpeed":@61,@"englishSpeed":@270};
    NSArray *inputRows=@[@{@"inputs":@20,@"corrected":@2,@"units":@20},@{@"inputs":@30,@"corrected":@3,@"units":@60},@{@"inputs":@0,@"corrected":@0,@"units":@0},@{@"inputs":@10,@"corrected":@2,@"units":@40},@{@"inputs":@25,@"corrected":@5,@"units":@180},@{@"inputs":@15,@"corrected":@3,@"units":@240}];
    delegate.statistics=@{@"input":@{@"chinese":inputRows,@"english":inputRows,@"cancelled":@7},@"versions":@[@"",@"0.8.0",@"0.9.0"],@"current":count,@"recent":count,@"previous":count,@"voice":count,@"bars":@[@{@"label":@"9/29",@"counts":count},@{@"label":@"9/30",@"counts":count},@{@"label":@"10/1",@"counts":count}]};
    delegate.syncStatus=@{@"lan":@{@"config":@{@"enabled":@YES,@"port":@50206,@"peers":@[@{@"id":@"peer-win",@"name":@"Windows",@"addresses":@[@"100.64.0.1:50206"]},@{@"id":@"peer-android",@"name":@"Pixel 9",@"addresses":@[]}]},@"status":@{@"message":@"已连接",@"port":@50206,@"online":@[@"peer-win"],@"connections":@{@"peer-win":@"100.64.0.1:49584"},@"addresses":@[@"192.168.1.20:50206",@"100.64.0.2:50206"],@"code":@"482910",@"expires":@((long long)(NSDate.date.timeIntervalSince1970+118)*1000)}}};
    RTSettings *settings=RTSettings.shared;[settings showWindow:nil];[settings.window makeKeyAndOrderFront:nil];
    CheckInputMenuSettings(settings);
    CheckAsyncSettings(settings);
    CheckDeletion();
    CheckTranslationCapture();
    NSStackView *navigation=[settings valueForKey:@"navigation"];
    [settings.window.contentView layoutSubtreeIfNeeded];
    for(NSButton *button in navigation.arrangedSubviews){
        Check(fabs(NSWidth(button.frame)-NSWidth(navigation.bounds))<1&&fabs(NSMinX(button.frame))<1,@"Navigation rows must fill the sidebar at a shared left edge");
        Check(fabs(NSMinX([button.cell titleRectForBounds:button.bounds])-38)<1,@"Navigation titles share a left inset");
    }
    NSString *output=[RTDataRoot() stringByAppendingPathComponent:@"screenshots"];[NSFileManager.defaultManager createDirectoryAtPath:output withIntermediateDirectories:YES attributes:nil error:NULL];
    for(NSInteger page=0;page<10;page++){
        [settings.window makeFirstResponder:nil];[settings setValue:@(page) forKey:@"page"];[settings refresh];
        NSView *body=[settings valueForKey:@"body"];NSTextField *header=[settings valueForKey:@"titleLabel"];NSRect titleFrame=header.frame;
        if(page==6){NSPopUpButton *versions=(NSPopUpButton *)Find(body,@"statistics-version");Check(versions.numberOfItems==4&&[versions.titleOfSelectedItem isEqual:@"全部版本"],@"Statistics defaults to all versions and includes unmarked history");}
        if(page==6){RTInputChart *ch=(RTInputChart *)Find(body,@"input-chart-chinese"),*en=(RTInputChart *)Find(body,@"input-chart-english");Check(ch.rows.count==6&&en.rows.count==6,@"Both language charts use six display groups");Check([[ch tooltipForIndex:4] containsString:@"7.2"],@"Tooltip means use actual units, not bucket midpoints");Check([[ch tooltipForIndex:2] containsString:@"回改率 —"],@"Empty buckets have unavailable rates");}
        if(page==6){for(NSString *identifier in @[@"input-chart-chinese",@"input-chart-english"]){RTInputChart *chart=(RTInputChart *)Find(body,identifier);for(NSUInteger mode=0;mode<2;mode++){chart.selectedIndex=mode==1?4:-1;NSBitmapImageRep *rep=[chart bitmapImageRepForCachingDisplayInRect:chart.bounds];[chart cacheDisplayInRect:chart.bounds toBitmapImageRep:rep];Check([[rep representationUsingType:NSBitmapImageFileTypePNG properties:@{}] writeToFile:[output stringByAppendingPathComponent:[NSString stringWithFormat:@"%@-%lu.png",identifier,(unsigned long)mode]] atomically:YES],@"Combo chart screenshot must be saved");}chart.selectedIndex=-1;}}
        if(page==8){
            [settings setValue:@YES forKey:@"lanJoining"];[settings setValue:@"peer-win" forKey:@"editingPeerID"];[settings setValue:@YES forKey:@"editingLanPort"];[settings refresh];
            Inspect(body);for(NSView *card in body.subviews)Check(NSMaxX(card.frame)<=NSWidth(body.bounds)-12,@"Expanded LAN controls must reserve scrollbar gutter");
            [settings setValue:@NO forKey:@"lanJoining"];[settings setValue:nil forKey:@"editingPeerID"];[settings setValue:@NO forKey:@"editingLanPort"];[settings refresh];
        }
        Inspect(body);for(NSView *card in body.subviews)Check(NSMaxX(card.frame)<=NSWidth(body.bounds)-12,@"Content must reserve its scrollbar gutter");
        NSScrollView *scroll=[settings valueForKey:@"scroll"];[scroll.contentView scrollToPoint:NSMakePoint(0,MAX(0,NSHeight(body.bounds)-100))];Check(NSEqualRects(header.frame,titleFrame),@"Page title must remain fixed while content scrolls");[scroll.contentView scrollToPoint:NSZeroPoint];
        [settings.window.contentView layoutSubtreeIfNeeded];[settings.window.contentView displayIfNeeded];
        NSBitmapImageRep *bitmap=[settings.window.contentView bitmapImageRepForCachingDisplayInRect:settings.window.contentView.bounds];
        Check(bitmap!=nil,@"Page must render to a bitmap");[settings.window.contentView cacheDisplayInRect:settings.window.contentView.bounds toBitmapImageRep:bitmap];
        NSData *png=[bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}];Check([png writeToFile:[output stringByAppendingPathComponent:[NSString stringWithFormat:@"page-%ld.png",(long)page]] atomically:YES],@"Screenshot must be saved");
    }
    [settings setValue:@2 forKey:@"page"];[settings refresh];NSTextField *field=(NSTextField *)Find(settings.window.contentView,@"provider.name");Check(field!=nil,@"Provider form exists");
    [settings.window makeFirstResponder:field];field.stringValue=@"正在编辑的名称";[(id<NSTextFieldDelegate>)settings controlTextDidChange:[NSNotification notificationWithName:NSControlTextDidChangeNotification object:field]];
    NSText *editor=field.currentEditor;
    Check(editor!=nil,@"Focused text field must have an active field editor");
    NSRect clipFrame=editor.superview.frame;
    Check(NSHeight(clipFrame)>=18.0&&fabs(NSMidY(clipFrame)-NSMidY(field.bounds))<=1.5,@"Active field editor clip view must be vertically centered without clipping characters");
    [RTSettings refreshIfLoaded];Check(Find(settings.window.contentView,@"provider.name")==field,@"Async status must preserve the active text editor");
    RTCandidateTestOwner *owner=[RTCandidateTestOwner new];RTCandidatePanel *candidate=RTCandidatePanel.shared;
    NSDictionary *state=@{@"candidates":@[@"这是一个较长的词组",@"你好"],@"generation":@1,@"selected":@0,@"pageStart":@0,@"pageSize":@2};
    [candidate showState:state anchor:NSMakeRect(100,200,1,20) screen:NSScreen.mainScreen level:0 owner:owner];
    Check(NSWidth(candidate.frame)<=480&&NSHeight(candidate.frame)==36,@"Candidate window is compact and bounded");Check(!candidate.canBecomeKeyWindow,@"Candidates cannot steal editor focus");[candidate hideFor:owner];
    [settings.window close];rt_resources_free(delegate.resources);delegate.resources=NULL;
    printf("All 10 settings pages rendered; form, scroll, editing and candidate checks passed.\n");
}return 0;}
