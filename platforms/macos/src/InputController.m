#import "App.h"

static NSEventModifierFlags RTFlagForKey(NSUInteger key){
    if(key==56||key==60)return NSEventModifierFlagShift;
    if(key==59||key==62)return NSEventModifierFlagControl;
    if(key==58||key==61)return NSEventModifierFlagOption;
    return 0;
}

@implementation RetypeInputController
- (void)dealloc {
    [self.timer invalidate];
    rt_session_free(self.session);
}
- (NSUInteger)recognizedEvents:(id)sender {
    (void)sender;
    return NSEventMaskKeyDown | NSEventMaskFlagsChanged;
}
- (void)activateServer:(id)sender {
    [super activateServer:sender];
    self.active = YES;
    self.activeClient = sender;
    self.epoch++;
    self.closers = NSMutableArray.array;
    ((RTAppDelegate *)NSApp.delegate).editor = self;
    self.chinese = ((RTAppDelegate *)NSApp.delegate).chinese;
    self.pendingTap = nil;
    if (self.session) [self applyCommand:@{@"type":@"reset"}];
    [self ensureSession];
}
- (void)deactivateServer:(id)sender {
    [self commitComposition:sender];
    if (self.session) RTSend(self.session, @{@"type":@"boundary"});
    self.epoch++;
    if (((RTAppDelegate *)NSApp.delegate).editor == self) ((RTAppDelegate *)NSApp.delegate).editor = nil;
    if (self.holdingVoice) [(RTAppDelegate *)NSApp.delegate voiceCancel];
    self.holdingVoice = NO;
    [self.closers removeAllObjects];
    self.active = NO;
    [self.timer invalidate]; self.timer = nil;
    if (self.session) RTSend(self.session, @{@"type":@"reset"});
    rt_session_free(self.session);
    self.session = NULL;
    [[RTCandidatePanel shared] hideFor:self];
    self.activeClient = nil;
    self.state = nil;
    [super deactivateServer:sender];
}
- (BOOL)ensureSession {
    RTAppDelegate *app = (RTAppDelegate *)NSApp.delegate;
    if (!self.session && app.runtime) {
        NSDictionary *input = RTPreferences.shared.value[@"input"];
        self.session = rt_session_new(app.runtime, [input[@"scheme"] boolValue], self.chinese);
    }
    if (self.session) {
        NSDictionary *input = RTPreferences.shared.value[@"input"];
        RTSend(self.session, @{@"type":@"configure", @"flypy":input[@"scheme"],
            @"english":input[@"english"], @"spelling":input[@"spelling"]});
    }
    return self.session != NULL;
}
- (BOOL)handleEvent:(NSEvent *)event client:(id)sender {
    if (!self.active) return NO;
    self.activeClient = sender;
    NSResponder *responder=NSApp.keyWindow.firstResponder;
    if([responder isKindOfClass:NSClassFromString(@"RTShortcutButton")]&&[[responder valueForKey:@"capturing"] boolValue])return NO;
    NSDictionary *shortcuts=RTPreferences.shared.value[@"shortcuts"];
    if (event.type == NSEventTypeFlagsChanged) {
        NSDictionary *voice=shortcuts[@"voice"];
        if([voice[@"tap"] boolValue]&&[voice[@"key"] unsignedIntValue]==event.keyCode){
            BOOL down=(event.modifierFlags&RTFlagForKey(event.keyCode))!=0;
            if(down&&!self.holdingVoice){[self commitComposition:sender];self.holdingVoice=YES;[(RTAppDelegate *)NSApp.delegate voiceStartForTest:NO];}
            else if(!down&&self.holdingVoice){self.holdingVoice=NO;[(RTAppDelegate *)NSApp.delegate voiceStop];}return NO;
        }
        for(NSString *role in @[@"mode",@"translate"]){NSDictionary *binding=shortcuts[role];
            if([binding[@"tap"] boolValue]&&[binding[@"key"] unsignedIntValue]==event.keyCode){
                if(event.modifierFlags&RTFlagForKey(event.keyCode))self.pendingTap=role;
                else if([self.pendingTap isEqual:role]){self.pendingTap=nil;[self shortcut:role];}return NO;
            }
        }
        self.pendingTap=nil;
        return NO;
    }
    self.pendingTap = nil;
    for(NSString *role in @[@"mode",@"translate",@"voice"]){NSDictionary *binding=shortcuts[role];
        if(![binding[@"tap"] boolValue]&&event.keyCode==[binding[@"key"] unsignedIntValue]&&RTModifiers(event.modifierFlags)==[binding[@"mods"] unsignedIntValue]){if(!event.isARepeat)[self shortcut:role];return YES;}
    }
    if(event.keyCode==53&&RTVoice.shared.running){[(RTAppDelegate *)NSApp.delegate voiceCancel];return YES;}
    self.epoch++;
    if(!(event.modifierFlags&(NSEventModifierFlagCommand|NSEventModifierFlagControl|NSEventModifierFlagOption))&&![self.state[@"composition"] length]&&[RTPreferences.shared.value[@"input"][@"symbols"] boolValue]){
        if([self pairSymbol:event.keyCode==51?@"backspace":event.characters])return YES;
    }
    if(event.keyCode>=123&&event.keyCode<=126)[self.closers removeAllObjects];
    // Keep macOS/app shortcuts, Option dead keys, and input-source switching native.
    if (event.modifierFlags & (NSEventModifierFlagCommand | NSEventModifierFlagControl | NSEventModifierFlagOption)) {
        [self commitComposition:sender];
        return NO;
    }
    if (!self.session && ![self ensureSession]) return NO;
    NSString *key;
    switch (event.keyCode) {
        case 51: key = @"backspace"; break; case 117: key = @"delete"; break;
        case 36: case 76: key = @"enter"; break; case 49: key = @"space"; break;
        case 53: key = @"escape"; break; case 48: key = @"tab"; break;
        case 123: key = @"left"; break; case 124: key = @"right"; break;
        case 125: key = @"down"; break; case 126: key = @"up"; break;
        case 115: key = @"home"; break; case 119: key = @"end"; break;
        case 116: key = @"pageUp"; break; case 121: key = @"pageDown"; break;
        default: key = event.characters; break;
    }
    if (key.length == 1 && [key characterAtIndex:0] >= 0xF700 && [key characterAtIndex:0] <= 0xF8FF) return NO;
    if (!key.length || (key.length == 1 && [key characterAtIndex:0] < 0x20)) return NO;
    NSDictionary *result = [self applyCommand:@{@"type":@"key", @"value":key,
        @"mods":@((event.modifierFlags & NSEventModifierFlagShift) ? 1 : 0)}];
    return result && ![result[@"passThrough"] boolValue] && !result[@"error"];
}
- (NSDictionary *)applyCommand:(NSDictionary *)command {
    if (!self.session || !self.activeClient) return nil;
    NSDictionary *result = RTSend(self.session, command);
    if (result[@"error"]) { NSLog(@"retype: input operation failed"); return result; }
    if ([command[@"type"] isEqual:@"poll"] && [result[@"commits"] count] == 0 &&
        [result[@"composition"] isEqual:self.state[@"composition"]] &&
        [result[@"candidates"] isEqual:self.state[@"candidates"]] &&
        [result[@"generation"] isEqual:self.state[@"generation"]] &&
        [result[@"selected"] isEqual:self.state[@"selected"]]) return result;
    NSDictionary *response = result;
    @try {
        for (NSString *text in result[@"commits"]) {
            [self.activeClient insertText:text replacementRange:NSMakeRange(NSNotFound, 0)];
        }
        if ([result[@"commits"] count]) RTSend(self.session, @{@"type":@"ack", @"token":result[@"token"], @"accepted":@YES});
        NSString *composition = result[@"composition"];
        NSString *previous = self.state[@"composition"] ?: @"";
        if (![composition isEqualToString:previous] || [result[@"commits"] count]) {
            [self.activeClient setMarkedText:composition selectionRange:NSMakeRange(composition.length, 0)
                replacementRange:NSMakeRange(NSNotFound, 0)];
        }
    } @catch (NSException *exception) {
        (void)exception;
        RTSend(self.session, @{@"type":@"ack", @"token":result[@"token"], @"accepted":@NO});
        RTSend(self.session, @{@"type":@"reset"});
        [[RTCandidatePanel shared] hideFor:self];
        NSLog(@"retype: client rejected marked text or commit");
        return @{ @"error": @"client rejected text" };
    }
    NSArray *candidates = result[@"candidates"];
    if (!candidates.count && ![result[@"composition"] length]) {
        self.state = result;
        self.chinese = [result[@"chinese"] boolValue];
        ((RTAppDelegate *)NSApp.delegate).chinese = self.chinese;
        [self.timer invalidate]; self.timer = nil;
        [[RTCandidatePanel shared] hideFor:self];
        return response;
    }
    NSRect anchor = NSZeroRect;
    @try { [self.activeClient attributesForCharacterIndex:0 lineHeightRectangle:&anchor]; }
    @catch (NSException *e) { (void)e; }
    NSScreen *screen = NSScreen.mainScreen;
    for (NSScreen *s in NSScreen.screens) if (NSIntersectsRect(s.frame, anchor)) { screen = s; break; }
    if (!screen) { [[RTCandidatePanel shared] hideFor:self]; return result; }
    if (NSIsEmptyRect(anchor)) anchor = NSMakeRect(NSMidX(screen.visibleFrame), NSMidY(screen.visibleFrame), 1, 18);
    if (candidates.count) {
        result = RTSend(self.session, @{@"type":@"layout", @"generation":result[@"generation"],
            @"widths":[RTCandidatePanel widths:candidates], @"available":@(MIN(472, MAX(80, NSWidth(screen.visibleFrame) - 16)))});
    }
    self.state = result;
    self.chinese = [result[@"chinese"] boolValue];
    ((RTAppDelegate *)NSApp.delegate).chinese = self.chinese;
    NSInteger level = NSNormalWindowLevel;
    @try { level = [self.activeClient windowLevel]; } @catch (NSException *e) { (void)e; }
    [[RTCandidatePanel shared] showState:result anchor:anchor screen:screen level:level owner:self];
    BOOL composing = [result[@"composition"] length] > 0;
    if (composing && !self.timer) {
        __weak RetypeInputController *weakSelf = self;
        self.timer = [NSTimer timerWithTimeInterval:0.03 repeats:YES block:^(NSTimer *timer) {
            (void)timer;
            RetypeInputController *controller = weakSelf;
            if (controller.active) [controller applyCommand:@{@"type":@"poll"}];
        }];
        [NSRunLoop.mainRunLoop addTimer:self.timer forMode:NSRunLoopCommonModes];
    } else if (!composing) { [self.timer invalidate]; self.timer = nil; }
    return response;
}
- (void)chooseCandidate:(NSUInteger)index generation:(uint64_t)generation {
    self.epoch++;
    if (self.active) [self applyCommand:@{@"type":@"choose", @"index":@(index), @"generation":@(generation)}];
}
- (void)commitComposition:(id)sender {
    (void)sender;
    if (self.active && [self.state[@"composition"] length]) [self applyCommand:@{@"type":@"key", @"value":@"enter"}];
}
- (NSMenu *)menu {
    NSMenu *menu = [[NSMenu alloc] initWithTitle:@"retype"];
    NSMenuItem *mode = [[NSMenuItem alloc] initWithTitle:self.chinese ? @"切换到英文" : @"切换到中文" action:@selector(toggleMode:) keyEquivalent:@""];
    mode.target = self;
    [menu addItem:mode];
    [menu addItem:NSMenuItem.separatorItem];
    NSMenuItem *settings = [[NSMenuItem alloc] initWithTitle:@"设置…" action:@selector(showSettings:) keyEquivalent:@""];
    settings.target = NSApp.delegate;
    [menu addItem:settings];
    return menu;
}
- (void)toggleMode:(id)sender { (void)sender; if (self.session) [self applyCommand:@{@"type":@"toggle"}]; }
- (void)shortcut:(NSString *)role {
    if([role isEqual:@"mode"]){if(!self.session)[self ensureSession];if(self.session)[self applyCommand:@{@"type":@"toggle"}];}
    else if([role isEqual:@"translate"]){[self commitComposition:self.activeClient];[(RTAppDelegate *)NSApp.delegate translate];}
    else if(RTVoice.shared.recording)[(RTAppDelegate *)NSApp.delegate voiceStop];
    else{[self commitComposition:self.activeClient];[(RTAppDelegate *)NSApp.delegate voiceStartForTest:NO];}
}
@end
