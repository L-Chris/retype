#import "App.h"
#import <ApplicationServices/ApplicationServices.h>

static id RTAXValue(AXUIElementRef element,CFStringRef attribute) {
    CFTypeRef value=NULL;AXError error=AXUIElementCopyAttributeValue(element,attribute,&value);
    return error==kAXErrorSuccess?CFBridgingRelease(value):nil;
}
static AXUIElementRef RTFocused(void) {
    if(!AXIsProcessTrusted())return NULL;
    AXUIElementRef system=AXUIElementCreateSystemWide();CFTypeRef element=NULL;
    AXUIElementSetMessagingTimeout(system,.3);
    AXError error=AXUIElementCopyAttributeValue(system,kAXFocusedUIElementAttribute,&element);CFRelease(system);
    if(error!=kAXErrorSuccess||!element)return NULL;
    AXUIElementSetMessagingTimeout((AXUIElementRef)element,.3);
    id subrole=RTAXValue((AXUIElementRef)element,kAXSubroleAttribute);
    if([subrole isEqual:(__bridge NSString *)kAXSecureTextFieldSubrole]){CFRelease(element);return NULL;}
    return (AXUIElementRef)element;
}
static BOOL RTAXRange(AXUIElementRef element,NSRange *range) {
    id raw=RTAXValue(element,kAXSelectedTextRangeAttribute);
    if(!raw||CFGetTypeID((__bridge CFTypeRef)raw)!=AXValueGetTypeID())return NO;
    CFRange r; if(!AXValueGetValue((__bridge AXValueRef)raw,kAXValueCFRangeType,&r)||r.location<0||r.length<0)return NO;
    *range=NSMakeRange((NSUInteger)r.location,(NSUInteger)r.length);return YES;
}
static BOOL RTAXSelect(AXUIElementRef element,NSRange range) {
    CFRange r=CFRangeMake((CFIndex)range.location,(CFIndex)range.length);AXValueRef value=AXValueCreate(kAXValueCFRangeType,&r);
    AXError result=AXUIElementSetAttributeValue(element,kAXSelectedTextRangeAttribute,value);CFRelease(value);return result==kAXErrorSuccess;
}
@implementation RetypeInputController (Editor)
- (NSUInteger)editorEpoch{return self.epoch;}
- (NSRect)anchorRect{NSRect rect=NSZeroRect;@try{[self.activeClient attributesForCharacterIndex:0 lineHeightRectangle:&rect];}@catch(NSException *e){(void)e;}return rect;}
- (NSDictionary *)captureText {
    if(!self.active||!self.activeClient)return @{ @"error":@"请先将光标放到输入框中" };
    @try {
        NSUInteger length=[(id)self.activeClient length];
        if(length>0&&length<=65536){NSRange actual=NSMakeRange(NSNotFound,0);NSString *text=[self.activeClient stringFromRange:NSMakeRange(0,length) actualRange:&actual];
            if(text.length==length&&actual.location==0&&actual.length==length)return @{@"text":text,@"client":self.activeClient,@"epoch":@(self.epoch),@"pid":@(NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier)};
        }
    }@catch(NSException *e){(void)e;}
    AXUIElementRef element=RTFocused();
    if(!element)return @{@"error":AXIsProcessTrusted()?@"应用不允许读取全部文字":@"请在系统设置中允许辅助功能后重试"};
    id value=RTAXValue(element,kAXValueAttribute);
    NSDictionary *result;
    if([value isKindOfClass:NSString.class]&&[value length]>0&&[value length]<=65536)result=@{@"text":value,@"ax":CFBridgingRelease(element),@"epoch":@(self.epoch),@"pid":@(NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier)};
    else{CFRelease(element);result=@{@"error":@"输入框为空或应用不允许读取文本"};}
    return result;
}
- (BOOL)replaceCapture:(NSDictionary *)capture text:(NSString *)text {
    if(!self.active||self.epoch!=[capture[@"epoch"] unsignedIntegerValue]||NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier!=[capture[@"pid"] intValue])return NO;
    @try{
        if(capture[@"client"]){
            if(capture[@"client"]!=self.activeClient)return NO;NSRange actual=NSMakeRange(NSNotFound,0);NSString *original=capture[@"text"];
            NSString *now=[self.activeClient stringFromRange:NSMakeRange(0,original.length) actualRange:&actual];
            if(![now isEqual:original]||[(id)self.activeClient length]!=original.length||actual.location!=0||actual.length!=original.length)return NO;
            [self.activeClient insertText:text replacementRange:NSMakeRange(0,original.length)];self.epoch++;return YES;
        }
        AXUIElementRef focused=RTFocused();if(!focused)return NO;
        AXUIElementRef saved=(__bridge AXUIElementRef)capture[@"ax"];
        BOOL same=CFEqual(saved,focused)&&[RTAXValue(focused,kAXValueAttribute) isEqual:capture[@"text"]];
        Boolean settable=false;if(same)AXUIElementIsAttributeSettable(focused,kAXValueAttribute,&settable);
        AXError result=(same&&settable)?AXUIElementSetAttributeValue(focused,kAXValueAttribute,(__bridge CFStringRef)text):kAXErrorCannotComplete;
        CFRelease(focused);if(result==kAXErrorSuccess)self.epoch++;return result==kAXErrorSuccess;
    }@catch(NSException *e){(void)e;return NO;}
}
- (BOOL)insertVoice:(NSString *)text epoch:(NSUInteger)epoch {
    if(!self.active||self.epoch!=epoch||!self.activeClient)return NO;
    @try{[self.activeClient insertText:text replacementRange:NSMakeRange(NSNotFound,0)];self.epoch++;
        RtRuntime *runtime=((RTAppDelegate *)NSApp.delegate).runtime;if(runtime)RTJSON(rt_runtime_voice(runtime,text.UTF8String));return YES;
    }@catch(NSException *e){(void)e;return NO;}
}
- (void)requestAccessibility:(id)sender{(void)sender;AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)@{(__bridge id)kAXTrustedCheckOptionPrompt:@YES});}
- (BOOL)pairSymbol:(NSString *)key {
    if(!key.length||(![key isEqual:@"backspace"]&&key.length!=1))return NO;
    AXUIElementRef element=RTFocused();if(!element)return NO;
    NSRange selection;Boolean settable=false;
    if(!RTAXRange(element,&selection)||AXUIElementIsAttributeSettable(element,kAXSelectedTextRangeAttribute,&settable)!=kAXErrorSuccess||!settable){CFRelease(element);return NO;}
    NSString *value=RTAXValue(element,kAXValueAttribute);
    if(![value isKindOfClass:NSString.class]||value.length>65536||selection.location>value.length||selection.length>value.length-selection.location){CFRelease(element);return NO;}
    NSString *before=selection.location?[value substringWithRange:NSMakeRange(selection.location-1,1)]:@"";
    NSString *after=NSMaxRange(selection)<value.length?[value substringWithRange:NSMakeRange(NSMaxRange(selection),1)]:@"";
    if([key isEqual:@"backspace"]){
        if(!selection.length&&self.closers.count&&[after isEqual:self.closers.lastObject]){
            NSDictionary *rules=RTFeature(@{@"type":@"symbolRules",@"key":before,@"chinese":@(self.chinese)})[@"value"];
            if([rules[@"right"] isEqual:after]&&selection.location){@try{[self.activeClient insertText:@"" replacementRange:NSMakeRange(selection.location-1,2)];[self.closers removeLastObject];CFRelease(element);return YES;}@catch(NSException *e){(void)e;}}
        }CFRelease(element);return NO;
    }
    NSDictionary *rules=RTFeature(@{@"type":@"symbolRules",@"key":key,@"chinese":@(self.chinese),@"before":before,@"after":after})[@"value"];
    NSString *close=rules[@"close"]==NSNull.null?nil:rules[@"close"];
    if(!selection.length&&close&&[close isEqual:after]&&[close isEqual:self.closers.lastObject]){BOOL moved=RTAXSelect(element,NSMakeRange(selection.location+1,0));if(moved)[self.closers removeLastObject];CFRelease(element);return moved;}
    if(![rules[@"pair"] boolValue]||selection.length){CFRelease(element);return NO;}
    NSString *left=rules[@"left"],*right=rules[@"right"];
    BOOL done=NO;
    @try{
        [self.activeClient insertText:[left stringByAppendingString:right] replacementRange:NSMakeRange(NSNotFound,0)];
        done=RTAXSelect(element,NSMakeRange(selection.location+left.length,0));
        if(done&&self.closers.count<16)[self.closers addObject:right];
        // A host which rejects caret placement must not receive the opener twice.
        if(!done)[RTToast.shared show:@"应用未允许移动光标" busy:NO];
        done=YES;
    }@catch(NSException *e){(void)e;}
    CFRelease(element);return done;
}
@end
