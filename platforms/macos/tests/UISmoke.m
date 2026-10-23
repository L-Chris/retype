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
    delegate.statistics=@{@"current":count,@"recent":count,@"previous":count,@"voice":count,@"bars":@[@{@"label":@"9/29",@"counts":count},@{@"label":@"9/30",@"counts":count},@{@"label":@"10/1",@"counts":count}]};
    RTSettings *settings=RTSettings.shared;[settings showWindow:nil];[settings.window makeKeyAndOrderFront:nil];
    NSString *output=[RTDataRoot() stringByAppendingPathComponent:@"screenshots"];[NSFileManager.defaultManager createDirectoryAtPath:output withIntermediateDirectories:YES attributes:nil error:NULL];
    for(NSInteger page=0;page<10;page++){
        [settings.window makeFirstResponder:nil];[settings setValue:@(page) forKey:@"page"];[settings refresh];
        NSView *body=[settings valueForKey:@"body"];NSTextField *header=[settings valueForKey:@"titleLabel"];NSRect titleFrame=header.frame;
        Inspect(body);for(NSView *card in body.subviews)Check(NSMaxX(card.frame)<=NSWidth(body.bounds)-12,@"Content must reserve its scrollbar gutter");
        NSScrollView *scroll=[settings valueForKey:@"scroll"];[scroll.contentView scrollToPoint:NSMakePoint(0,MAX(0,NSHeight(body.bounds)-100))];Check(NSEqualRects(header.frame,titleFrame),@"Page title must remain fixed while content scrolls");[scroll.contentView scrollToPoint:NSZeroPoint];
        [settings.window.contentView layoutSubtreeIfNeeded];[settings.window.contentView displayIfNeeded];
        NSBitmapImageRep *bitmap=[settings.window.contentView bitmapImageRepForCachingDisplayInRect:settings.window.contentView.bounds];
        Check(bitmap!=nil,@"Page must render to a bitmap");[settings.window.contentView cacheDisplayInRect:settings.window.contentView.bounds toBitmapImageRep:bitmap];
        NSData *png=[bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}];Check([png writeToFile:[output stringByAppendingPathComponent:[NSString stringWithFormat:@"page-%ld.png",(long)page]] atomically:YES],@"Screenshot must be saved");
    }
    [settings setValue:@2 forKey:@"page"];[settings refresh];NSTextField *field=(NSTextField *)Find(settings.window.contentView,@"provider.name");Check(field!=nil,@"Provider form exists");
    [settings.window makeFirstResponder:field];field.stringValue=@"正在编辑的名称";[(id<NSTextFieldDelegate>)settings controlTextDidChange:[NSNotification notificationWithName:NSControlTextDidChangeNotification object:field]];
    [RTSettings refreshIfLoaded];Check(Find(settings.window.contentView,@"provider.name")==field,@"Async status must preserve the active text editor");
    RTCandidateTestOwner *owner=[RTCandidateTestOwner new];RTCandidatePanel *candidate=RTCandidatePanel.shared;
    NSDictionary *state=@{@"candidates":@[@"这是一个较长的词组",@"你好"],@"generation":@1,@"selected":@0,@"pageStart":@0,@"pageSize":@2};
    [candidate showState:state anchor:NSMakeRect(100,200,1,20) screen:NSScreen.mainScreen level:0 owner:owner];
    Check(NSWidth(candidate.frame)<=480&&NSHeight(candidate.frame)==36,@"Candidate window is compact and bounded");Check(!candidate.canBecomeKeyWindow,@"Candidates cannot steal editor focus");[candidate hideFor:owner];
    [settings.window close];rt_resources_free(delegate.resources);delegate.resources=NULL;
    printf("All 10 settings pages rendered; form, scroll, editing and candidate checks passed.\n");
}return 0;}
