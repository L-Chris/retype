#import "App.h"
#import <objc/runtime.h>
#import <ApplicationServices/ApplicationServices.h>
#import <QuartzCore/QuartzCore.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>

static NSColor *RTInk(void) { return [NSColor colorWithCalibratedRed:20/255. green:43/255. blue:59/255. alpha:1]; }
static NSColor *RTAccent(void) { return [NSColor colorWithCalibratedRed:19/255. green:143/255. blue:150/255. alpha:1]; }
static NSColor *RTMuted(void) { return [NSColor colorWithCalibratedRed:101/255. green:119/255. blue:129/255. alpha:1]; }
static NSTextField *RTLabel(NSString *text, CGFloat size) {
    NSTextField *label = [NSTextField labelWithString:text ?: @""];
    label.font = [NSFont systemFontOfSize:size]; label.textColor = RTInk(); label.selectable = NO;
    return label;
}
// Metadata only: no field values, provider URLs, credentials or input text.
static void RTSettingsLog(NSString *event, NSInteger page, CFAbsoluteTime started, BOOL success) {
    double elapsed=MAX(0,(CFAbsoluteTimeGetCurrent()-started)*1000);
    NSDictionary *entry=@{@"time":@(NSDate.date.timeIntervalSince1970),@"event":event,@"page":@(page),@"duration_ms":@(elapsed),@"success":@(success)};
    static dispatch_queue_t queue;static dispatch_once_t once;
    dispatch_once(&once,^{queue=dispatch_queue_create("retype.settings.diagnostics",DISPATCH_QUEUE_SERIAL);});
    dispatch_async(queue,^{
        NSString *directory=[RTDataRoot() stringByAppendingPathComponent:@"logs"];
        NSFileManager *files=NSFileManager.defaultManager;
        [files createDirectoryAtPath:directory withIntermediateDirectories:YES attributes:@{NSFilePosixPermissions:@0700} error:NULL];
        NSString *path=[directory stringByAppendingPathComponent:@"settings-performance.jsonl"];
        if([[files attributesOfItemAtPath:path error:NULL][NSFileSize] unsignedLongLongValue]>=262144){
            NSString *previous=[path stringByAppendingString:@".previous"];
            [files removeItemAtPath:previous error:NULL];[files moveItemAtPath:path toPath:previous error:NULL];
        }
        if(![files fileExistsAtPath:path])[files createFileAtPath:path contents:nil attributes:@{NSFilePosixPermissions:@0600}];
        NSFileHandle *handle=[NSFileHandle fileHandleForWritingAtPath:path];
        @try{[handle seekToEndOfFile];[handle writeData:[NSJSONSerialization dataWithJSONObject:entry options:0 error:NULL]];[handle writeData:[@"\n" dataUsingEncoding:NSUTF8StringEncoding]];}@catch(NSException *e){(void)e;}@finally{[handle closeFile];}
    });
}
static void RTUpdateSecretFields(NSView *view,NSString *path,NSString *value) {
    if([view isKindOfClass:NSSecureTextField.class]&&[view.identifier isEqual:path]){
        NSSecureTextField *field=(NSSecureTextField *)view;field.enabled=value!=nil;
        field.stringValue=value?:@"";field.placeholderString=value?@"":@"无法读取，请重新打开此页重试";
    }
    for(NSView *child in view.subviews)RTUpdateSecretFields(child,path,value);
}
@interface RTFlipped : NSView @end
@implementation RTFlipped
- (BOOL)isFlipped { return YES; }
@end
@interface RTNavigationCell : NSButtonCell @end
@implementation RTNavigationCell
- (NSRect)imageRectForBounds:(NSRect)bounds {
    return NSMakeRect(NSMinX(bounds)+12,NSMidY(bounds)-9,18,18);
}
- (NSRect)titleRectForBounds:(NSRect)bounds {
    NSRect rect=[super titleRectForBounds:bounds];
    rect.origin.x=NSMinX(bounds)+38;rect.size.width=MAX(0,NSWidth(bounds)-50);
    return rect;
}
@end
@interface RTLightScroller : NSScroller @end
@implementation RTLightScroller
+ (CGFloat)scrollerWidthForControlSize:(NSControlSize)size scrollerStyle:(NSScrollerStyle)style { (void)size; (void)style; return 8; }
- (void)drawKnob {
    [[NSColor colorWithCalibratedWhite:0.80 alpha:0.65] setFill];
    NSRect rect = NSInsetRect([self rectForPart:NSScrollerKnob], 2, 1);
    [[NSBezierPath bezierPathWithRoundedRect:rect xRadius:2 yRadius:2] fill];
}
- (void)drawKnobSlotInRect:(NSRect)rect highlight:(BOOL)highlight { (void)rect; (void)highlight; }
@end
@interface RTVerticalCell : NSTextFieldCell @end
@implementation RTVerticalCell
- (NSRect)drawingRectForBounds:(NSRect)rect {
    NSRect r = [super drawingRectForBounds:rect];
    CGFloat h = self.cellSize.height; r.origin.y += MAX(0, (r.size.height - h) / 2); r.size.height = MIN(h, r.size.height);
    return r;
}
@end
@interface RTShortcutButton : NSButton
@property(nonatomic, copy) void (^captured)(NSDictionary *);
@property(nonatomic) BOOL capturing;
@property(nonatomic) unsigned short modifierKey;
@end
@implementation RTShortcutButton
- (BOOL)acceptsFirstResponder { return YES; }
- (void)mouseDown:(NSEvent *)event {
    (void)event; self.capturing = YES; self.title = @"请按快捷键…"; [self.window makeFirstResponder:self];
}
- (void)keyDown:(NSEvent *)event {
    if (!self.capturing) { [super keyDown:event]; return; }
    if (event.keyCode == 53) { self.capturing = NO; [self.window makeFirstResponder:nil]; [RTSettings refreshIfLoaded]; return; }
    if (self.captured) self.captured(@{@"key":@(event.keyCode),@"mods":@(RTModifiers(event.modifierFlags)),@"tap":@NO});
    self.capturing = NO; [self.window makeFirstResponder:nil];
}
- (void)flagsChanged:(NSEvent *)event {
    if (!self.capturing) return;
    if (RTModifiers(event.modifierFlags)) self.modifierKey = event.keyCode;
    else if (self.modifierKey && self.captured) {
        self.captured(@{@"key":@(self.modifierKey),@"mods":@0,@"tap":@YES});
        self.capturing = NO; self.modifierKey = 0; [self.window makeFirstResponder:nil];
    }
}
@end
@interface RTChart : NSView
@property(nonatomic, copy) NSArray *bars;
@property(nonatomic, copy) NSArray<NSString *> *tooltips;
@end
@implementation RTChart
- (BOOL)isFlipped { return YES; }
- (void)setBars:(NSArray *)bars {
    _bars = bars; [self removeAllToolTips];
    NSMutableArray<NSString *> *tooltips = NSMutableArray.array;
    CGFloat step = NSWidth(self.bounds) / MAX(1, bars.count);
    for (NSUInteger i = 0; i < bars.count; i++) {
        NSDictionary *bar = bars[i], *c = bar[@"counts"];
        NSString *tip = [NSString stringWithFormat:@"%@\n中文 %@ 字/分钟\n英文 %@ 词/分钟",bar[@"label"],c[@"chineseSpeed"] == NSNull.null ? @"—" : c[@"chineseSpeed"],c[@"englishSpeed"] == NSNull.null ? @"—" : c[@"englishSpeed"]];
        [tooltips addObject:tip];
        [self addToolTipRect:NSMakeRect(i * step, 0, step, NSHeight(self.bounds)) owner:self userData:(void *)(uintptr_t)i];
    }
    self.tooltips = tooltips;
    self.needsDisplay = YES;
}
- (NSString *)view:(NSView *)view stringForToolTip:(NSToolTipTag)tag point:(NSPoint)point userData:(void *)data {
    (void)view; (void)tag; (void)point; NSUInteger index = (NSUInteger)(uintptr_t)data;
    return index < self.tooltips.count ? self.tooltips[index] : @"";
}
- (void)drawRect:(NSRect)rect {
    (void)rect;
    CGFloat maximum = 1;
    for (NSDictionary *bar in self.bars) for (NSString *key in @[@"chineseSpeed",@"englishSpeed"]) if (bar[@"counts"][key] != NSNull.null) maximum = MAX(maximum,[bar[@"counts"][key] doubleValue]);
    CGFloat step = NSWidth(self.bounds) / MAX(1,self.bars.count), height = NSHeight(self.bounds) - 28;
    for (NSUInteger i = 0; i < self.bars.count; i++) {
        NSDictionary *bar = self.bars[i], *c = bar[@"counts"];
        for (NSUInteger j = 0; j < 2; j++) {
            id n = c[j == 0 ? @"chineseSpeed" : @"englishSpeed"];
            if (n == NSNull.null || [n doubleValue] <= 0) continue;
            CGFloat h = [n doubleValue] / maximum * (height - 8), width = MIN(12,step * .32);
            [(j == 0 ? RTAccent() : [NSColor colorWithCalibratedRed:.43 green:.72 blue:.81 alpha:1]) setFill];
            [[NSBezierPath bezierPathWithRoundedRect:NSMakeRect(i*step+step*.15+j*width,height-h,width-1,h) xRadius:2 yRadius:2] fill];
        }
        NSUInteger stride = self.bars.count > 14 ? 4 : 1;
        if (i % stride == 0) [bar[@"label"] drawAtPoint:NSMakePoint(i*step+2,height+8) withAttributes:@{NSFontAttributeName:[NSFont systemFontOfSize:10],NSForegroundColorAttributeName:RTMuted()}];
    }
}
@end

@interface RTSettings () <NSWindowDelegate,NSTextFieldDelegate,NSComboBoxDelegate>
@property(nonatomic) NSInteger page;
@property(nonatomic) NSInteger period;
@property(nonatomic) NSInteger chineseInputSelection;
@property(nonatomic) NSInteger englishInputSelection;
@property(nonatomic, strong) RTFlipped *body;
@property(nonatomic, strong) NSScrollView *scroll;
@property(nonatomic, strong) NSTextField *titleLabel;
@property(nonatomic, strong) NSTextField *messageLabel;
@property(nonatomic, strong) NSStackView *navigation;
@property(nonatomic, strong) NSView *card;
@property(nonatomic) CGFloat y;
@property(nonatomic) CGFloat row;
@property(nonatomic, copy) NSString *providerID;
@property(nonatomic, copy) NSString *testModel;
@property(nonatomic, strong) NSArray *presets;
@property(nonatomic, strong) NSArray *packs;
@property(nonatomic, strong) NSTimer *progressTimer;
@property(nonatomic, strong) NSTimer *pageTimer;
@property(nonatomic) BOOL lanLoading;
@property(nonatomic, strong) NSProgressIndicator *progress;
@property(nonatomic, strong) NSMutableDictionary *secretValues;
@property(nonatomic, strong) NSMutableDictionary<NSString *,NSNumber *> *secretVersions;
@property(nonatomic, strong) NSMutableSet<NSString *> *secretLoading;
@property(nonatomic, strong) dispatch_queue_t secretQueue;
@property(nonatomic, strong) NSTimer *performanceTimer;
@end
static RTSettings *RTLoadedSettings;
@implementation RTSettings
+ (void)refreshIfLoaded {
    NSResponder *responder=RTLoadedSettings.window.firstResponder;
    if([responder isKindOfClass:NSTextView.class]&&[(NSTextView *)responder isFieldEditor]) {
        RTLoadedSettings.messageLabel.stringValue=((RTAppDelegate *)NSApp.delegate).messages[@(RTLoadedSettings.page)]?:@"";
        return;
    }
    [RTLoadedSettings refresh];
}
+ (instancetype)shared {
    static RTSettings *controller; static dispatch_once_t once;
    dispatch_once(&once, ^{
        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,840,660)
            styleMask:NSWindowStyleMaskTitled|NSWindowStyleMaskClosable|NSWindowStyleMaskMiniaturizable|NSWindowStyleMaskFullSizeContentView
            backing:NSBackingStoreBuffered defer:NO];
        window.title = @"retype 设置"; window.titleVisibility = NSWindowTitleHidden; window.titlebarAppearsTransparent = YES;
        window.appearance = [NSAppearance appearanceNamed:NSAppearanceNameAqua]; window.releasedWhenClosed = NO;
        window.movableByWindowBackground = YES; [window center];
        controller = [[self alloc] initWithWindow:window]; window.delegate = controller; RTLoadedSettings = controller;
        [controller setup];
    }); return controller;
}
- (void)setup {
    self.chineseInputSelection=-1;self.englishInputSelection=-1;
    self.secretValues=NSMutableDictionary.dictionary;self.secretVersions=NSMutableDictionary.dictionary;self.secretLoading=NSMutableSet.set;
    self.secretQueue=RTSecretQueue();
    NSView *root = self.window.contentView; root.wantsLayer = YES;
    root.layer.backgroundColor = [NSColor colorWithCalibratedRed:246/255. green:249/255. blue:250/255. alpha:1].CGColor;
    NSView *sidebar = [[NSView alloc] initWithFrame:NSMakeRect(0,0,190,660)]; sidebar.wantsLayer = YES;
    sidebar.layer.backgroundColor = [NSColor colorWithCalibratedRed:222/255. green:247/255. blue:249/255. alpha:1].CGColor;
    [root addSubview:sidebar];
    NSImageView *logo = [[NSImageView alloc] initWithFrame:NSMakeRect(20,588,36,36)]; logo.image = [NSImage imageNamed:@"logo"]; [sidebar addSubview:logo];
    NSTextField *brand = RTLabel(@"retype 输入法",15); brand.frame = NSMakeRect(66,593,120,24); [sidebar addSubview:brand];
    NSArray *titles = @[@"输入",@"词库",@"AI 提供商",@"翻译",@"语音输入",@"快捷键",@"统计",@"云同步",@"跨设备粘贴",@"关于"];
    NSArray *icons = @[@"keyboard",@"books.vertical",@"cpu",@"character.bubble",@"mic",@"command",@"chart.bar",@"cloud",@"doc.on.clipboard",@"info.circle"];
    self.navigation = [[NSStackView alloc] initWithFrame:NSMakeRect(12,68,166,490)];
    self.navigation.orientation = NSUserInterfaceLayoutOrientationVertical; self.navigation.spacing = 6; self.navigation.distribution = NSStackViewDistributionFillEqually;
    self.navigation.alignment = NSLayoutAttributeLeading;
    [sidebar addSubview:self.navigation];
    for (NSUInteger i = 0; i < titles.count; i++) {
        NSButton *button = [NSButton new];button.cell=[RTNavigationCell new];
        button.title=titles[i];button.image=[NSImage imageWithSystemSymbolName:icons[i] accessibilityDescription:titles[i]];button.target=self;button.action=@selector(navigate:);
        button.tag = (NSInteger)i; button.bordered = NO; button.alignment = NSTextAlignmentLeft; button.imagePosition = NSImageLeft;
        button.font = [NSFont systemFontOfSize:14]; button.contentTintColor = RTInk(); button.wantsLayer = YES; button.layer.cornerRadius = 10;
        [self.navigation addArrangedSubview:button];
        button.translatesAutoresizingMaskIntoConstraints=NO;
        [button.widthAnchor constraintEqualToAnchor:self.navigation.widthAnchor].active=YES;
    }
    self.titleLabel = RTLabel(@"输入",24); self.titleLabel.frame = NSMakeRect(222,585,560,38); [root addSubview:self.titleLabel];
    self.scroll = [[NSScrollView alloc] initWithFrame:NSMakeRect(212,56,608,512)];
    self.scroll.hasVerticalScroller = YES; self.scroll.autohidesScrollers = NO; self.scroll.scrollerStyle = NSScrollerStyleOverlay;
    self.scroll.verticalScroller = [RTLightScroller new]; self.scroll.drawsBackground = NO;
    self.body = [[RTFlipped alloc] initWithFrame:NSMakeRect(0,0,580,510)]; self.scroll.documentView = self.body; [root addSubview:self.scroll];
    self.messageLabel = RTLabel(@"",12); self.messageLabel.textColor = RTMuted(); self.messageLabel.frame = NSMakeRect(232,16,570,32);
    self.messageLabel.maximumNumberOfLines = 2; self.messageLabel.lineBreakMode = NSLineBreakByWordWrapping; [root addSubview:self.messageLabel];
    self.presets = RTFeature(@{@"type":@"providerPresets"})[@"value"];
    self.packs = RTFeature(@{@"type":@"packCatalog"})[@"value"];
    [self refresh];
}
- (void)windowWillClose:(NSNotification *)notification {
    (void)notification; [self.progressTimer invalidate]; self.progressTimer = nil;
    [self.performanceTimer invalidate];self.performanceTimer=nil;
    [self.pageTimer invalidate]; self.pageTimer = nil;
    [(RTAppDelegate *)NSApp.delegate voiceCancel];
}
- (void)showWindow:(id)sender {
    if (!self.window.visible) {((RTAppDelegate *)NSApp.delegate).statisticsVersion=nil;self.chineseInputSelection=-1;self.englishInputSelection=-1;}
    [super showWindow:sender]; [self startPageUpdates];
    if(!self.performanceTimer){
        __weak RTSettings *weakSelf=self;__block CFAbsoluteTime previous=CFAbsoluteTimeGetCurrent();
        self.performanceTimer=[NSTimer timerWithTimeInterval:.25 repeats:YES block:^(NSTimer *timer){
            RTSettings *self=weakSelf;if(!self){[timer invalidate];return;}
            CFAbsoluteTime now=CFAbsoluteTimeGetCurrent();
            if(now-previous>.40)RTSettingsLog(@"main_thread_delay",self.page,previous+.25,YES);
            previous=now;
        }];
        [NSRunLoop.mainRunLoop addTimer:self.performanceTimer forMode:NSRunLoopCommonModes];
    }
}
- (void)startPageUpdates {
    [self.pageTimer invalidate]; self.pageTimer = nil;
    if (self.page != 6 && self.page != 8) return;
    __weak RTSettings *weakSelf = self;
    self.pageTimer = [NSTimer scheduledTimerWithTimeInterval:self.page == 6 ? 5 : 1 repeats:YES block:^(NSTimer *timer) {
        RTSettings *self = weakSelf;
        if (!self || !self.window.visible) { [timer invalidate]; return; }
        RTAppDelegate *app = (RTAppDelegate *)NSApp.delegate;
        if (self.page == 6 && ![app.jobs containsObject:@"statistics"]) [app loadStatistics:self.period];
        if (self.page == 8 && !self.lanLoading) [self loadLAN];
    }];
    RTAppDelegate *app = (RTAppDelegate *)NSApp.delegate;
    if (self.page == 6) [app loadStatistics:self.period];
    if (self.page == 8) [self loadLAN];
}
- (NSMutableDictionary *)provider {
    NSArray *providers = RTPreferences.shared.value[@"ai"][@"providers"];
    for (NSMutableDictionary *p in providers) if ([p[@"id"] isEqual:self.providerID]) return p;
    NSMutableDictionary *p = providers.firstObject; self.providerID = p[@"id"]; return p;
}
- (id)read:(NSString *)path {
    if ([path hasPrefix:@"secret:"]) return self.secretValues[[path substringFromIndex:7]];
    if ([path isEqual:@"testModel"]) return self.testModel ?: [[self provider][@"models"] firstObject];
    if ([path hasPrefix:@"provider."]) return [[self provider] valueForKeyPath:[path substringFromIndex:9]];
    return [RTPreferences.shared.value valueForKeyPath:path];
}
- (void)write:(NSString *)path value:(id)value {
    if ([path hasPrefix:@"secret:"]) {
        NSString *account=[path substringFromIndex:7],*text=[value copy];
        if([self.secretValues[account] isEqual:text])return;
        NSUInteger version=[self.secretVersions[account] unsignedIntegerValue]+1;NSInteger page=self.page;
        self.secretVersions[account]=@(version);self.secretValues[account]=text;
        dispatch_async(self.secretQueue,^{
            CFAbsoluteTime started=CFAbsoluteTimeGetCurrent();BOOL success=RTSaveSecret(account,text);
            RTSettingsLog(@"keychain_write",page,started,success);
            dispatch_async(dispatch_get_main_queue(),^{
                if(!success&&[self.secretVersions[account] unsignedIntegerValue]==version){
                    [self.secretValues removeObjectForKey:account];
                    ((RTAppDelegate *)NSApp.delegate).messages[@(page)]=@"无法保存到钥匙串，请解锁后重试";
                    if(self.page==page)self.messageLabel.stringValue=@"无法保存到钥匙串，请解锁后重试";
                }
            });
        });return;
    }
    if ([path isEqual:@"testModel"]) {
        self.testModel = value; NSMutableDictionary *p = [self provider];
        if ([value length] && ![p[@"models"] containsObject:value]) [p[@"models"] addObject:value];
    } else if ([path hasPrefix:@"provider."]) [[self provider] setValue:value forKeyPath:[path substringFromIndex:9]];
    else [RTPreferences.shared.value setValue:value forKeyPath:path];
    [RTPreferences.shared save];
}
- (NSView *)newCard {
    RTFlipped *card = [[RTFlipped alloc] initWithFrame:NSMakeRect(10,self.y,556,60)];
    card.wantsLayer = YES; card.layer.backgroundColor = NSColor.whiteColor.CGColor; card.layer.cornerRadius = 12;
    card.layer.borderWidth = 1; card.layer.borderColor = [NSColor colorWithCalibratedRed:227/255. green:233/255. blue:236/255. alpha:1].CGColor;
    [self.body addSubview:card]; self.card = card; self.row = 18; return card;
}
- (void)finishCard {
    NSRect frame = self.card.frame; frame.size.height = self.row + 12; self.card.frame = frame;
    self.y += frame.size.height + 18;
}
- (void)label:(NSString *)text size:(CGFloat)size {
    NSTextField *label = RTLabel(text,size); label.frame = NSMakeRect(20,self.row,516,28); [self.card addSubview:label]; self.row += 38;
}
- (NSTextField *)rowLabel:(NSString *)title {
    NSTextField *label = RTLabel(title,14); label.frame = NSMakeRect(20,self.row+7,112,22); [self.card addSubview:label]; return label;
}
- (void)loadSecret:(NSString *)path field:(NSSecureTextField *)field {
    NSString *account=[path substringFromIndex:7];id cached=self.secretValues[account];
    if(cached){RTUpdateSecretFields(field,path,[cached isKindOfClass:NSString.class]?cached:nil);return;}
    field.enabled=NO;field.placeholderString=@"正在读取钥匙串…";
    if([self.secretLoading containsObject:account])return;
    [self.secretLoading addObject:account];NSUInteger version=[self.secretVersions[account] unsignedIntegerValue];NSInteger page=self.page;
    dispatch_async(self.secretQueue,^{
        CFAbsoluteTime started=CFAbsoluteTimeGetCurrent();NSString *value=RTSecret(account);
        RTSettingsLog(@"keychain_read",page,started,value!=nil);
        dispatch_async(dispatch_get_main_queue(),^{
            [self.secretLoading removeObject:account];
            if([self.secretVersions[account] unsignedIntegerValue]!=version)return;
            self.secretValues[account]=value?:NSNull.null;
            RTUpdateSecretFields(self.body,path,value);
        });
    });
}
- (NSTextField *)field:(NSString *)title path:(NSString *)path secure:(BOOL)secure {
    [self rowLabel:title];
    NSTextField *field = secure ? [[NSSecureTextField alloc] initWithFrame:NSMakeRect(140,self.row,392,34)] : [[NSTextField alloc] initWithFrame:NSMakeRect(140,self.row,392,34)];
    if (!secure) field.cell = [RTVerticalCell new];
    field.font = [NSFont systemFontOfSize:14]; field.identifier = path; field.delegate = self;
    field.controlSize = NSControlSizeLarge;
    field.editable=YES;field.selectable=YES;field.bezeled=YES;field.cell.usesSingleLineMode=YES;field.cell.scrollable=YES;
    if(secure)[self loadSecret:path field:(NSSecureTextField *)field];
    else{ id value=[self read:path];field.stringValue=[value isKindOfClass:NSString.class]?value:[value description]?:@""; }
    field.bezelStyle = NSTextFieldRoundedBezel; field.drawsBackground = YES; field.backgroundColor = NSColor.whiteColor;
    [self.card addSubview:field]; self.row += 50; return field;
}
- (NSPopUpButton *)popup:(NSString *)title path:(NSString *)path labels:(NSArray<NSString *> *)labels values:(NSArray *)values action:(SEL)action {
    [self rowLabel:title]; NSPopUpButton *popup = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(140,self.row,392,34) pullsDown:NO];
    popup.identifier = path; popup.font = [NSFont systemFontOfSize:14]; popup.target = self; popup.action = action ?: @selector(select:);
    popup.controlSize = NSControlSizeLarge;
    for (NSUInteger i = 0; i < labels.count; i++) { [popup addItemWithTitle:labels[i]]; popup.lastItem.representedObject = values[i]; if ([[self read:path] isEqual:values[i]]) [popup selectItem:popup.lastItem]; }
    [self.card addSubview:popup]; self.row += 50; return popup;
}
- (void)toggle:(NSString *)title path:(NSString *)path {
    NSTextField *label=[self rowLabel:title];[label setFrameSize:NSMakeSize(440,22)]; NSSwitch *toggle = [[NSSwitch alloc] initWithFrame:NSMakeRect(484,self.row+3,48,28)];
    toggle.identifier = path; toggle.state = [[self read:path] boolValue] ? NSControlStateValueOn : NSControlStateValueOff;
    toggle.target = self; toggle.action = @selector(toggle:); [self.card addSubview:toggle]; self.row += 48;
}
- (NSButton *)button:(NSString *)title action:(SEL)action x:(CGFloat)x width:(CGFloat)width {
    NSButton *button = [NSButton buttonWithTitle:title target:self action:action];
    button.frame = NSMakeRect(x,self.row,width,34); button.font = [NSFont systemFontOfSize:13]; button.bezelStyle = NSBezelStyleRounded;
    [self.card addSubview:button]; return button;
}
- (void)modelRow:(BOOL)voice {
    NSString *providerPath = voice ? @"ai.voice.provider" : @"ai.provider", *modelPath = voice ? @"ai.voice.model" : @"ai.model";
    NSMutableArray *labels = NSMutableArray.array, *values = NSMutableArray.array;
    for (NSDictionary *p in RTPreferences.shared.value[@"ai"][@"providers"]) {
        if (voice && ![@[@"Compatible",@"Gemini"] containsObject:p[@"kind"]]) continue;
        [labels addObject:p[@"name"]]; [values addObject:p[@"id"]];
    }
    if (!labels.count) { [self label:@"先在 AI 提供商中添加提供商和模型。" size:14]; return; }
    if (![values containsObject:[self read:providerPath] ?: @""]) [self write:providerPath value:values.firstObject];
    [self popup:@"提供商" path:providerPath labels:labels values:values action:@selector(modelProvider:)];
    NSArray *models = @[];
    for (NSDictionary *p in RTPreferences.shared.value[@"ai"][@"providers"]) if ([p[@"id"] isEqual:[self read:providerPath]]) { models = p[@"models"]; break; }
    if (models.count) {
        if (![models containsObject:[self read:modelPath] ?: @""]) [self write:modelPath value:models.firstObject];
        [self popup:@"模型" path:modelPath labels:models values:models action:@selector(select:)];
    }
    else [self label:@"该提供商尚未添加模型。" size:13];
}
- (NSString *)cloudAccount { return RTFeature(@{@"type":@"cloudAccount",@"config":RTPreferences.shared.value[@"cloud"]})[@"value"] ?: @""; }
- (void)refresh {
    if (!self.body) return;
    CFAbsoluteTime started=CFAbsoluteTimeGetCurrent();
    RTAppDelegate *app = (RTAppDelegate *)NSApp.delegate;
    for (NSView *view in self.body.subviews.copy) [view removeFromSuperview];
    self.y = 0; self.card = nil;
    NSArray *titles = @[@"输入",@"词库",@"AI 提供商",@"翻译",@"语音输入",@"快捷键",@"统计",@"云同步",@"跨设备粘贴",@"关于"];
    self.titleLabel.stringValue = titles[self.page];
    for (NSButton *button in self.navigation.arrangedSubviews) button.layer.backgroundColor = button.tag == self.page ? NSColor.whiteColor.CGColor : NSColor.clearColor.CGColor;
    self.messageLabel.stringValue = app.messages[@(self.page)] ?: @"";
    if (self.page == 0) {
        [self newCard]; [self label:@"拼音方案" size:16];
        [self popup:@"输入方式" path:@"input.scheme" labels:@[@"全拼",@"小鹤双拼"] values:@[@0,@1] action:nil];
        [self finishCard]; [self newCard]; [self toggle:@"英文单词补全" path:@"input.english"]; [self toggle:@"英文拼写建议" path:@"input.spelling"]; [self finishCard];
        [self newCard]; [self toggle:@"符号自动补全" path:@"input.symbols"]; [self finishCard];
        if(!AXIsProcessTrusted()){[self newCard];[self button:@"允许辅助功能访问…" action:@selector(accessibility:) x:20 width:190];self.row+=46;[self finishCard];}
    } else if (self.page == 1) {
        if (!app.runtime || app.resourceBusy) {
            [self newCard]; [self label:@"安装基础词库后即可离线输入" size:16];
            [self button:app.resourceBusy ? @"取消下载" : @"下载词库" action:app.resourceBusy ? @selector(cancelResources:) : @selector(downloadResources:) x:20 width:136];
            [self button:@"导入本地词库包…" action:@selector(importResources:) x:170 width:172].enabled = !app.resourceBusy;
            self.row += 48;
            if (app.resourceBusy) { self.progress = [[NSProgressIndicator alloc] initWithFrame:NSMakeRect(20,self.row,512,6)]; self.progress.indeterminate = NO; self.progress.maxValue = 100; [self.card addSubview:self.progress]; self.row += 18; }
            [self label:app.resourceMessage size:12]; [self finishCard];
        }
        for (NSUInteger i = 0; i < self.packs.count; i++) {
            NSDictionary *p = self.packs[i]; [self newCard]; [self label:p[@"title"] size:16];
            NSTextField *description = RTLabel([NSString stringWithFormat:@"%@ · %.1f MB",p[@"description"],[p[@"bytes"] doubleValue]/1048576],12);
            description.textColor = RTMuted(); description.frame = NSMakeRect(20,self.row,440,24); [self.card addSubview:description]; self.row += 30;
            NSSwitch *toggle = [[NSSwitch alloc] initWithFrame:NSMakeRect(484,24,48,28)]; toggle.tag = (NSInteger)i; toggle.target = self; toggle.action = @selector(pack:);
            toggle.state = ([RTPreferences.shared.value[@"packs"] unsignedIntegerValue] & (1 << i)) ? NSControlStateValueOn : NSControlStateValueOff;
            toggle.enabled = ![app.jobs containsObject:@"pack"]; [self.card addSubview:toggle]; [self finishCard];
        }
    } else if (self.page == 2) {
        [self newCard]; NSArray *providers = RTPreferences.shared.value[@"ai"][@"providers"];
        if (providers.count) {
            [self provider]; NSMutableArray *names = NSMutableArray.array,*ids = NSMutableArray.array;
            for (NSDictionary *p in providers) { [names addObject:p[@"name"]]; [ids addObject:p[@"id"]]; }
            NSPopUpButton *picker = [self popup:@"提供商" path:@"providerID" labels:names values:ids action:@selector(providerChanged:)];
            for (NSMenuItem *item in picker.itemArray) if ([item.representedObject isEqual:self.providerID]) [picker selectItem:item];
        }
        [self button:@"添加提供商" action:@selector(addProvider:) x:140 width:132];
        if (providers.count) [self button:@"删除" action:@selector(removeProvider:) x:286 width:86];
        self.row += 48; [self finishCard];
        if (providers.count) {
            [self newCard];
            NSMutableArray *names = NSMutableArray.array; for (NSDictionary *preset in self.presets) [names addObject:preset[@"name"]];
            [self popup:@"预设" path:@"provider.preset" labels:names values:names action:@selector(preset:)];
            [self field:@"名称" path:@"provider.name" secure:NO];
            [self popup:@"接口类型" path:@"provider.kind" labels:@[@"OpenAI 兼容",@"Anthropic",@"Gemini",@"Ollama"] values:@[@"Compatible",@"Anthropic",@"Gemini",@"Ollama"] action:nil];
            [self field:@"接口地址" path:@"provider.base_url" secure:NO];
            [self field:@"API Key" path:[@"secret:ai/" stringByAppendingString:self.providerID] secure:YES];
            [self rowLabel:@"模型"]; NSComboBox *model = [[NSComboBox alloc] initWithFrame:NSMakeRect(140,self.row,392,34)];
            model.identifier = @"testModel"; model.font = [NSFont systemFontOfSize:14]; model.delegate = self; [model addItemsWithObjectValues:[self provider][@"models"]];
            model.stringValue = self.testModel ?: [[self provider][@"models"] firstObject] ?: @""; [self.card addSubview:model]; self.row += 50;
            [self button:@"刷新模型" action:@selector(refreshModels:) x:140 width:120].enabled = ![app.jobs containsObject:@"provider"];
            [self button:@"测试连接" action:@selector(testProvider:) x:274 width:120].enabled = ![app.jobs containsObject:@"provider"];
            self.row += 46; [self finishCard];
        }
    } else if (self.page == 3) {
        [self newCard]; [self modelRow:NO];
        [self field:@"翻译语言" path:@"ai.target" secure:NO];
        [self popup:@"思考等级" path:@"ai.reasoning" labels:@[@"不思考",@"模型默认",@"极低",@"低",@"中",@"高"] values:@[@"none",@"default",@"minimal",@"low",@"medium",@"high"] action:nil];
        [self field:@"补充要求" path:@"ai.instructions" secure:NO];
        [self field:@"超时（秒）" path:@"ai.timeout_seconds" secure:NO];
        [self toggle:@"先预览译文" path:@"ai.preview"]; [self finishCard];
    } else if (self.page == 4) {
        [self newCard]; [self modelRow:YES];
        [self popup:@"语言" path:@"ai.voice.language" labels:@[@"自动识别",@"中文",@"英文"] values:@[@"auto",@"zh",@"en"] action:nil];
        NSMutableDictionary *provider = nil; NSString *model = RTPreferences.shared.value[@"ai"][@"voice"][@"model"];
        for (NSMutableDictionary *p in RTPreferences.shared.value[@"ai"][@"providers"]) if ([p[@"id"] isEqual:RTPreferences.shared.value[@"ai"][@"voice"][@"provider"]]) provider = p;
        if (provider && model.length) {
            NSString *transport = provider[@"voice_protocols"][model];
            if (!transport) transport = [provider[@"kind"] isEqual:@"Gemini"] && [model isEqual:@"gemini-3.5-transcribe-live"] ? @"GeminiLive" : @"File";
            NSPopUpButton *protocol = [self popup:@"识别方式" path:@"voiceTransport" labels:@[@"录完后识别",@"实时预览（Gemini Live）"] values:@[@"File",@"GeminiLive"] action:@selector(voiceProtocol:)];
            for (NSMenuItem *item in protocol.itemArray) if ([item.representedObject isEqual:transport]) [protocol selectItem:item];
        }
        [self toggle:@"整理口语" path:@"ai.voice.tidy"]; [self finishCard];
        [self newCard]; [self label:@"录音测试" size:16];
        NSButton *record = [self button:RTVoice.shared.running ? (RTVoice.shared.recording ? @"停止录音" : @"识别中…") : @"开始录音" action:@selector(testVoice:) x:20 width:128];
        record.enabled = !RTVoice.shared.running || RTVoice.shared.recording;
        if (RTVoice.shared.running) [self button:@"取消" action:@selector(cancelVoice:) x:164 width:88]; self.row += 48;
        if (RTVoice.shared.preview.length) { NSTextField *preview = RTLabel(RTVoice.shared.preview,14); preview.maximumNumberOfLines = 8; preview.lineBreakMode = NSLineBreakByWordWrapping; preview.frame = NSMakeRect(20,self.row,512,140); [self.card addSubview:preview]; self.row += 150; }
        [self finishCard];
    } else if (self.page == 5) {
        [self newCard];
        NSArray *roles = @[@"mode",@"translate",@"voice"],*names = @[@"切换中英文",@"翻译",@"语音输入"];
        for (NSUInteger i = 0; i < roles.count; i++) {
            [self rowLabel:names[i]]; RTShortcutButton *button = [[RTShortcutButton alloc] initWithFrame:NSMakeRect(140,self.row,392,34)];
            button.title = RTShortcutLabel(RTPreferences.shared.value[@"shortcuts"][roles[i]]); button.bezelStyle = NSBezelStyleRounded;
            NSString *role = roles[i]; __weak RTSettings *weakSelf = self;
            button.captured = ^(NSDictionary *binding) {
                NSMutableDictionary *keys = [RTPreferences.shared.value[@"shortcuts"] mutableCopy]; keys[role] = binding;
                NSDictionary *result = RTFeature(@{@"type":@"validateShortcuts",@"shortcuts":keys});
                if (result[@"error"]) app.messages[@5] = result[@"error"];
                else { RTPreferences.shared.value[@"shortcuts"] = keys; [RTPreferences.shared save]; app.messages[@5] = @""; }
                [weakSelf refresh];
            }; [self.card addSubview:button]; self.row += 50;
        } [self finishCard];
    } else if (self.page == 6) {
        NSDictionary *s = app.statistics;
        [self newCard]; [self rowLabel:@"版本"];
        NSPopUpButton *versions=[[NSPopUpButton alloc] initWithFrame:NSMakeRect(140,self.row,392,30) pullsDown:NO];
        [versions addItemWithTitle:@"全部版本"]; versions.lastItem.representedObject=NSNull.null;
        for (NSString *version in s[@"versions"]) {
            [versions addItemWithTitle:version.length?version:@"历史未标记版本"];versions.lastItem.representedObject=version;
            if (app.statisticsVersion && [app.statisticsVersion isEqualToString:version]) [versions selectItem:versions.lastItem];
        }
        versions.identifier=@"statistics-version";versions.target=self;versions.action=@selector(statisticsVersion:);[self.card addSubview:versions];self.row+=44;[self finishCard];
        [self newCard]; [self label:@"最近 5 分钟" size:16];
        [self metric:s[@"recent"] previous:nil]; [self finishCard];
        [self newCard]; [self label:@"平均速度" size:16];
        NSButton *help = [NSButton buttonWithImage:[NSImage imageWithSystemSymbolName:@"questionmark.circle" accessibilityDescription:@"平均速度说明"] target:nil action:NULL];
        help.bordered = NO; help.toolTip = @"均速 = 中文字数或英文单词数 ÷ 有效输入时间；样本不足时不显示速度。";
        help.frame = NSMakeRect(96,20,22,22); [self.card addSubview:help];
        NSSegmentedControl *period = [NSSegmentedControl segmentedControlWithLabels:@[@"天",@"周",@"月",@"年"] trackingMode:NSSegmentSwitchTrackingSelectOne target:self action:@selector(period:)];
        period.frame = NSMakeRect(20,self.row,220,32); period.selectedSegment = self.period; [self.card addSubview:period]; self.row += 46;
        [self metric:s[@"current"] previous:s[@"previous"]];
        RTChart *chart = [[RTChart alloc] initWithFrame:NSMakeRect(20,self.row,512,168)]; chart.bars = s[@"bars"] ?: @[]; [self.card addSubview:chart]; self.row += 180;
        [self finishCard];
        [self newCard]; [self label:@"回改与上屏长度" size:16];
        NSButton *inputHelp=[NSButton buttonWithImage:[NSImage imageWithSystemSymbolName:@"questionmark.circle" accessibilityDescription:@"输入分布说明"] target:nil action:NULL];inputHelp.bordered=NO;inputHelp.frame=NSMakeRect(160,20,22,22);inputHelp.toolTip=[NSString stringWithFormat:@"当前时间与版本范围内取消输入 %@ 次。取消没有最终长度，不分配到图中。\n仅包含新口径启用后采集的样本。",s[@"input"][@"cancelled"]?:@0];[self.card addSubview:inputHelp];
        [self finishCard];
        for(NSUInteger i=0;i<2;i++){
            [self newCard];[self label:i==0?@"中文输入 · 字/次":@"英文输入 · 词/次" size:16];
            RTInputChart *inputChart=[[RTInputChart alloc] initWithFrame:NSMakeRect(20,self.row,512,220)];inputChart.unit=i==0?@"字":@"词";inputChart.rows=s[@"input"][i==0?@"chinese":@"english"]?:@[];inputChart.selectedIndex=i==0?self.chineseInputSelection:self.englishInputSelection;inputChart.identifier=i==0?@"input-chart-chinese":@"input-chart-english";
            __weak RTSettings *weakSelf=self;inputChart.selectionChanged=^(NSInteger index){if(i==0)weakSelf.chineseInputSelection=index;else weakSelf.englishInputSelection=index;};
            NSButton *summary=[NSButton buttonWithImage:[NSImage imageWithSystemSymbolName:@"questionmark.circle" accessibilityDescription:@"输入统计详情"] target:nil action:NULL];summary.bordered=NO;summary.frame=NSMakeRect(174,20,22,22);summary.toolTip=[NSString stringWithFormat:@"本%@ · %@\n%@",@[@"天",@"周",@"月",@"年"][self.period],app.statisticsVersion?(app.statisticsVersion.length?app.statisticsVersion:@"历史未标记版本"):@"全部版本",inputChart.summary];[self.card addSubview:summary];[self.card addSubview:inputChart];self.row+=232;[self finishCard];
        }
        [self newCard]; [self label:@"语音输入" size:16];
        [self label:[NSString stringWithFormat:@"中文 %@ 字 · 英文 %@ 词",s[@"voice"][@"chinese"] ?: @0,s[@"voice"][@"english"] ?: @0] size:14];
        [self finishCard];
    } else if (self.page == 7) {
        [self newCard]; [self toggle:@"云同步" path:@"cloud.enabled"];
        [self popup:@"云盘" path:@"cloud.provider" labels:@[@"坚果云",@"cstcloud",@"InfiniCLOUD",@"Koofr",@"HiDrive",@"Yandex Disk",@"自定义"] values:@[@"坚果云",@"cstcloud",@"InfiniCLOUD",@"Koofr",@"HiDrive",@"Yandex Disk",@"自定义"] action:@selector(cloudPreset:)];
        [self field:@"WebDAV 地址" path:@"cloud.url" secure:NO]; [self field:@"用户名" path:@"cloud.username" secure:NO];
        [self field:@"应用密码" path:[@"secret:sync/" stringByAppendingString:[self cloudAccount]] secure:YES];
        [self field:@"设备名称" path:@"cloud.device_name" secure:NO];
        [self button:@"测试连接" action:@selector(testCloud:) x:140 width:120].enabled = ![app.jobs containsObject:@"cloud"];
        [self button:[app.syncStatus[@"awaitingMerge"] boolValue] ? @"确认首次合并" : @"立即同步" action:@selector(syncCloud:) x:274 width:144].enabled = ![app.jobs containsObject:@"cloud"];
        self.row += 48; [self finishCard];
        for (NSDictionary *conflict in app.syncStatus[@"conflicts"]) {
            [self newCard]; [self label:[@"设置冲突：" stringByAppendingString:conflict[@"key"]] size:14];
            NSButton *local = [self button:@"使用本机" action:@selector(resolveCloud:) x:20 width:128]; local.identifier = @"local"; local.toolTip = conflict[@"key"];
            objc_setAssociatedObject(local,@selector(resolveCloud:),conflict,OBJC_ASSOCIATION_RETAIN_NONATOMIC);
            NSButton *remote = [self button:@"使用云端" action:@selector(resolveCloud:) x:164 width:128]; remote.identifier = @"remote";
            objc_setAssociatedObject(remote,@selector(resolveCloud:),conflict,OBJC_ASSOCIATION_RETAIN_NONATOMIC);
            self.row += 48; [self finishCard];
        }
    } else if (self.page == 8) {
        NSDictionary *lan = app.syncStatus[@"lan"] ?: @{};
        [self newCard]; [self rowLabel:@"跨设备粘贴"];
        NSSwitch *toggle = [[NSSwitch alloc] initWithFrame:NSMakeRect(484,self.row+3,48,28)]; toggle.target=self;toggle.action=@selector(lanEnable:);
        toggle.state=[lan[@"config"][@"enabled"] boolValue]?NSControlStateValueOn:NSControlStateValueOff;[self.card addSubview:toggle];self.row+=48;
        [self button:@"查看匹配码" action:@selector(pairCode:) x:20 width:140];
        [self button:@"关联设备…" action:@selector(joinDevice:) x:176 width:140];self.row+=48;
        if ([lan[@"status"][@"code"] length]) [self label:lan[@"status"][@"code"] size:26];
        if ([lan[@"status"][@"message"] length]) [self label:lan[@"status"][@"message"] size:12];
        [self finishCard];
        for (NSDictionary *peer in lan[@"config"][@"peers"]) {
            [self newCard];[self label:peer[@"name"] size:15];NSButton *remove=[self button:@"取消关联" action:@selector(removePeer:) x:20 width:120];remove.identifier=peer[@"id"];self.row+=44;[self finishCard];
        }
    } else {
        [self newCard];
        NSString *version = NSBundle.mainBundle.infoDictionary[@"CFBundleShortVersionString"] ?: @"";
        [self label:[@"retype 输入法  " stringByAppendingString:version] size:18];
        [self toggle:@"每天自动检查并提醒" path:@"autoCheck"];
        [self button:@"检查更新" action:@selector(checkUpdate:) x:20 width:116].enabled = ![app.jobs containsObject:@"updates"];
        if (app.update[@"version"]) {
            [self button:@"下载并安装" action:@selector(installUpdate:) x:150 width:130].enabled = ![app.jobs containsObject:@"updates"];
            [self button:@"跳过此版本" action:@selector(skipUpdate:) x:294 width:130];
        }
        self.row += 46; [self finishCard]; [self newCard];
        [self button:@"项目主页" action:@selector(home:) x:20 width:116];
        [self button:@"开源许可" action:@selector(licenses:) x:150 width:116];
        [self button:@"数据目录" action:@selector(dataFolder:) x:280 width:116]; self.row += 46; [self finishCard];
    }
    self.body.frame = NSMakeRect(0,0,580,MAX(510,self.y));
    if (app.resourceBusy && self.page == 1 && !self.progressTimer) {
        __weak RTSettings *weakSelf = self;
        self.progressTimer = [NSTimer scheduledTimerWithTimeInterval:.25 repeats:YES block:^(NSTimer *t){(void)t;weakSelf.progress.doubleValue=rt_resources_progress(app.resources)/10.;}];
    } else if (!app.resourceBusy || self.page != 1) { [self.progressTimer invalidate];self.progressTimer=nil; }
    RTSettingsLog(@"page_render",self.page,started,YES);
}
- (void)metric:(NSDictionary *)counts previous:(NSDictionary *)previous {
    for (NSUInteger i=0;i<2;i++) {
        CGFloat x=20+i*260;NSString *speedKey=i?@"englishSpeed":@"chineseSpeed";id speed=counts[speedKey];
        NSTextField *name=RTLabel(i?@"英文":@"中文",13);name.textColor=RTMuted();name.frame=NSMakeRect(x,self.row,240,22);[self.card addSubview:name];
        NSTextField *number=RTLabel(speed&&speed!=NSNull.null?[speed description]:@"—",28);number.frame=NSMakeRect(x,self.row+28,240,38);[self.card addSubview:number];
        NSTextField *unit=RTLabel(i?@"词/分钟":@"字/分钟",12);unit.textColor=RTMuted();unit.frame=NSMakeRect(x,self.row+70,240,20);[self.card addSubview:unit];
        NSNumber *old=previous[speedKey];NSString *comparison=@"上期无可比数据";
        if (speed&&speed!=NSNull.null&&old&&(id)old!=NSNull.null&&old.doubleValue>0)comparison=[NSString stringWithFormat:@"较上期 %+.0f%%",([speed doubleValue]/old.doubleValue-1)*100];
        if(previous){NSTextField *compare=RTLabel(comparison,12);compare.textColor=RTMuted();compare.frame=NSMakeRect(x,self.row+96,240,20);[self.card addSubview:compare];}
        NSTextField *count=RTLabel([NSString stringWithFormat:@"%@ %@ · 有效 %.1f 分钟",counts[i?@"english":@"chinese"]?:@0,i?@"词":@"字",[counts[i?@"englishMs":@"chineseMs"] doubleValue]/60000],12);
        count.textColor=RTMuted();count.frame=NSMakeRect(x,self.row+(previous?124:98),248,22);[self.card addSubview:count];
    }
    self.row += previous?158:132;
}
- (void)navigate:(NSButton *)sender {
    CFAbsoluteTime started=CFAbsoluteTimeGetCurrent();
    [self.window makeFirstResponder:nil];
    // Re-read on explicit navigation so cloud changes and permission retries are
    // reflected, while background refreshes reuse the current page's values.
    [self.secretValues removeAllObjects];
    self.page = sender.tag; [self.scroll.contentView scrollToPoint:NSZeroPoint]; [self refresh];
    [self startPageUpdates];
    RTSettingsLog(@"page_switch",self.page,started,YES);
}
- (void)controlTextDidChange:(NSNotification *)notification {
    NSTextField *field=notification.object;NSString *path=field.identifier;if(!path)return;
    if ([path hasPrefix:@"secret:"] && (!field.enabled||![notification.name isEqual:NSControlTextDidEndEditingNotification])) return;
    id value=field.stringValue;
    if([path isEqual:@"ai.timeout_seconds"]){NSInteger seconds=field.integerValue;if(seconds<5||seconds>180)return;value=@(seconds);}
    [self write:path value:value];
}
- (void)controlTextDidEndEditing:(NSNotification *)notification {[self controlTextDidChange:notification];}
- (void)comboBoxSelectionDidChange:(NSNotification *)notification {[self controlTextDidChange:notification];}
- (void)select:(NSPopUpButton *)sender {[self write:sender.identifier value:sender.selectedItem.representedObject];if([sender.identifier isEqual:@"ai.voice.model"])[self refresh];}
- (void)toggle:(NSSwitch *)sender {[self write:sender.identifier value:@(sender.state==NSControlStateValueOn)];if([sender.identifier isEqual:@"cloud.enabled"]&&sender.state==NSControlStateValueOn)[(RTAppDelegate *)NSApp.delegate syncCloud:NO confirm:NO resolution:nil];}
- (void)providerChanged:(NSPopUpButton *)sender {self.providerID=sender.selectedItem.representedObject;self.testModel=nil;[self refresh];}
- (void)addProvider:(id)sender {
    (void)sender;NSMutableDictionary *p=[@{@"id":NSUUID.UUID.UUIDString,@"preset":@"自定义",@"name":@"自定义",@"kind":@"Compatible",@"base_url":@"",@"models":NSMutableArray.array,@"voice_protocols":NSMutableDictionary.dictionary} mutableCopy];
    [RTPreferences.shared.value[@"ai"][@"providers"] addObject:p];self.providerID=p[@"id"];self.testModel=nil;[RTPreferences.shared save];[self refresh];
}
- (void)removeProvider:(id)sender {
    (void)sender;NSMutableDictionary *p=[self provider];[self write:[@"secret:ai/" stringByAppendingString:p[@"id"]] value:@""];
    [RTPreferences.shared.value[@"ai"][@"providers"] removeObject:p];self.providerID=nil;self.testModel=nil;[RTPreferences.shared save];[self refresh];
}
- (void)preset:(NSPopUpButton *)sender {
    for(NSDictionary *preset in self.presets)if([preset[@"name"] isEqual:sender.selectedItem.representedObject]){
        NSMutableDictionary *p=[self provider];p[@"preset"]=preset[@"name"];p[@"name"]=preset[@"name"];p[@"base_url"]=preset[@"url"];p[@"kind"]=preset[@"kind"];break;
    }
    [RTPreferences.shared save];[self refresh];
}
- (void)modelProvider:(NSPopUpButton *)sender {
    [self select:sender];BOOL voice=[sender.identifier containsString:@"voice"];NSString *model=@"";
    for(NSDictionary *p in RTPreferences.shared.value[@"ai"][@"providers"])if([p[@"id"] isEqual:sender.selectedItem.representedObject])model=[p[@"models"] firstObject]?:@"";
    [self write:voice?@"ai.voice.model":@"ai.model" value:model];[self refresh];
}
- (void)providerJob:(BOOL)test {
    [self.window makeFirstResponder:nil];RTAppDelegate *app=(RTAppDelegate *)NSApp.delegate;if([app.jobs containsObject:@"provider"])return;
    NSDictionary *p=[[self provider] copy];NSString *id=p[@"id"];
    NSUInteger revision=RTPreferences.shared.revision;[app.jobs addObject:@"provider"];app.messages[@2]=test?@"测试中…":@"正在获取模型…";[self refresh];
    [app feature:@{@"type":test?@"testProvider":@"models",@"provider":p,@"model":self.testModel?:[p[@"models"] firstObject]?:@""} secretAccount:[@"ai/" stringByAppendingString:id] field:@"key" completion:^(NSDictionary *r){
        [app.jobs removeObject:@"provider"];app.messages[@2]=r[@"error"]?:@"连接成功";
        if(!test&&!r[@"error"]&&revision==RTPreferences.shared.revision){
            NSMutableDictionary *current=[self provider];
            if([current[@"id"] isEqual:id]){NSMutableOrderedSet *models=[NSMutableOrderedSet orderedSetWithArray:current[@"models"]];[models addObjectsFromArray:r[@"value"]];current[@"models"]=[models.array mutableCopy];[RTPreferences.shared save];app.messages[@2]=@"模型已更新";}
        }[RTSettings refreshIfLoaded];
    }];
}
- (void)refreshModels:(id)sender{(void)sender;[self providerJob:NO];}
- (void)testProvider:(id)sender{(void)sender;[self providerJob:YES];}
- (void)voiceProtocol:(NSPopUpButton *)sender {
    NSDictionary *voice=RTPreferences.shared.value[@"ai"][@"voice"];
    for(NSMutableDictionary *p in RTPreferences.shared.value[@"ai"][@"providers"])if([p[@"id"] isEqual:voice[@"provider"]]){
        if(!p[@"voice_protocols"])p[@"voice_protocols"]=NSMutableDictionary.dictionary;p[@"voice_protocols"][voice[@"model"]]=sender.selectedItem.representedObject;
    }
    [RTPreferences.shared save];
}
- (void)testVoice:(id)sender{(void)sender;if(RTVoice.shared.running)[(RTAppDelegate *)NSApp.delegate voiceStop];else [(RTAppDelegate *)NSApp.delegate voiceStartForTest:YES];}
- (void)cancelVoice:(id)sender{(void)sender;[(RTAppDelegate *)NSApp.delegate voiceCancel];}
- (void)statisticsVersion:(NSPopUpButton *)sender {self.chineseInputSelection=-1;self.englishInputSelection=-1; RTAppDelegate *app=(RTAppDelegate *)NSApp.delegate;id version=sender.selectedItem.representedObject;app.statisticsVersion=version==NSNull.null?nil:version;[app loadStatistics:self.period]; }
- (void)period:(NSSegmentedControl *)sender{self.chineseInputSelection=-1;self.englishInputSelection=-1;self.period=sender.selectedSegment;[(RTAppDelegate *)NSApp.delegate loadStatistics:self.period];}
- (void)pack:(NSSwitch *)sender {
    RTAppDelegate *app=(RTAppDelegate *)NSApp.delegate;NSUInteger index=sender.tag,mask=[RTPreferences.shared.value[@"packs"] unsignedIntegerValue];BOOL enabled=sender.state==NSControlStateValueOn;
    if(!enabled){RTPreferences.shared.value[@"packs"]=@(mask&~(1<<index));[RTPreferences.shared save];[app reloadRuntime];return;}
    [app.jobs addObject:@"pack"];app.messages[@1]=@"正在安装词库…";[self refresh];
    [app feature:@{@"type":@"installPack",@"id":self.packs[index][@"id"]} completion:^(NSDictionary *r){
        [app.jobs removeObject:@"pack"];app.messages[@1]=r[@"error"]?:@"";
        if(!r[@"error"]){NSUInteger current=[RTPreferences.shared.value[@"packs"] unsignedIntegerValue];RTPreferences.shared.value[@"packs"]=@(current|(1<<index));[RTPreferences.shared save];[app reloadRuntime];}
        [RTSettings refreshIfLoaded];
    }];
}
- (void)downloadResources:(id)sender{(void)sender;[(RTAppDelegate *)NSApp.delegate runResources:1 source:nil];}
- (void)cancelResources:(id)sender{(void)sender;rt_resources_cancel(((RTAppDelegate *)NSApp.delegate).resources);}
- (void)importResources:(id)sender{
    (void)sender;NSOpenPanel *panel=NSOpenPanel.openPanel;panel.canChooseDirectories=NO;panel.allowsMultipleSelection=NO;
    panel.allowedContentTypes=@[[UTType typeWithFilenameExtension:@"retype-resources"]?:UTTypeData];
    [panel beginSheetModalForWindow:self.window completionHandler:^(NSModalResponse result){if(result==NSModalResponseOK)[(RTAppDelegate *)NSApp.delegate runResources:2 source:panel.URL.path];}];
}
- (void)cloudPreset:(NSPopUpButton *)sender{
    [self select:sender];NSDictionary *urls=@{@"坚果云":@"https://dav.jianguoyun.com/dav/",@"cstcloud":@"https://data.cstcloud.cn/dav/",@"Koofr":@"https://app.koofr.net/dav/Koofr/",@"HiDrive":@"https://webdav.hidrive.strato.com/"};NSString *url=urls[sender.selectedItem.title];if(url)[self write:@"cloud.url" value:url];[self refresh];
}
- (void)testCloud:(id)sender{(void)sender;[self.window makeFirstResponder:nil];[(RTAppDelegate *)NSApp.delegate syncCloud:YES confirm:NO resolution:nil];}
- (void)syncCloud:(id)sender{(void)sender;[self.window makeFirstResponder:nil];RTAppDelegate *app=(RTAppDelegate *)NSApp.delegate;[app syncCloud:NO confirm:[app.syncStatus[@"awaitingMerge"] boolValue] resolution:nil];}
- (void)resolveCloud:(NSButton *)sender{NSDictionary *c=objc_getAssociatedObject(sender,@selector(resolveCloud:));[(RTAppDelegate *)NSApp.delegate syncCloud:NO confirm:YES resolution:@{@"resolveKey":c[@"key"],@"resolveStamp":c[@"remote"][@"stamp"],@"useRemote":@([sender.identifier isEqual:@"remote"])}];}
- (void)checkUpdate:(id)sender{(void)sender;[(RTAppDelegate *)NSApp.delegate checkUpdate:YES];}
- (void)installUpdate:(id)sender{(void)sender;[(RTAppDelegate *)NSApp.delegate installUpdate];}
- (void)skipUpdate:(id)sender{(void)sender;RTAppDelegate *app=(RTAppDelegate *)NSApp.delegate;RTPreferences.shared.value[@"skippedVersion"]=app.update[@"version"]?:@"";[RTPreferences.shared save];app.update=nil;app.messages[@9]=@"已跳过此版本";[self refresh];}
- (void)home:(id)sender{(void)sender;[NSWorkspace.sharedWorkspace openURL:[NSURL URLWithString:@"https://github.com/L-Chris/retype"]];}
- (void)licenses:(id)sender{(void)sender;[NSWorkspace.sharedWorkspace openURL:[NSBundle.mainBundle URLForResource:@"NOTICE" withExtension:@"txt"]];}
- (void)dataFolder:(id)sender{(void)sender;[NSWorkspace.sharedWorkspace openURL:[NSURL fileURLWithPath:RTDataRoot()]];}
- (void)loadLAN {
    if (self.lanLoading) return; self.lanLoading = YES;
    RTAppDelegate *app=(RTAppDelegate *)NSApp.delegate;
    [app feature:@{@"type":@"lanStatus"} completion:^(NSDictionary *r){self.lanLoading=NO;if(!r[@"error"]){if([app.syncStatus[@"lan"] isEqual:r[@"value"]])return;NSMutableDictionary *s=[app.syncStatus mutableCopy]?:NSMutableDictionary.dictionary;s[@"lan"]=r[@"value"];app.syncStatus=s;}else app.messages[@8]=r[@"error"];if(self.page==8)[RTSettings refreshIfLoaded];}];
}
- (void)lanCommand:(id)command{RTAppDelegate *app=(RTAppDelegate *)NSApp.delegate;[app feature:@{@"type":@"lanCommand",@"command":command} completion:^(NSDictionary *r){app.messages[@8]=r[@"error"]?:@"";dispatch_after(dispatch_time(DISPATCH_TIME_NOW,NSEC_PER_SEC),dispatch_get_main_queue(),^{[self loadLAN];});}];}
- (void)lanEnable:(NSSwitch *)sender{[self lanCommand:@{@"Enable":@(sender.state==NSControlStateValueOn)}];}
- (void)pairCode:(id)sender{(void)sender;[self lanCommand:@"Pair"];}
- (void)joinDevice:(id)sender{
    (void)sender;NSAlert *alert=[NSAlert new];alert.messageText=@"关联设备";alert.informativeText=@"输入另一台设备上显示的 6 位匹配码。";
    NSTextField *field=[[NSTextField alloc] initWithFrame:NSMakeRect(0,0,260,34)];alert.accessoryView=field;[alert addButtonWithTitle:@"关联"];[alert addButtonWithTitle:@"取消"];
    [alert beginSheetModalForWindow:self.window completionHandler:^(NSModalResponse r){if(r==NSAlertFirstButtonReturn)[self lanCommand:@{@"Join":field.stringValue}];}];
}
- (void)removePeer:(NSButton *)sender{[self lanCommand:@{@"Remove":sender.identifier}];}
- (void)accessibility:(id)sender{(void)sender;AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)@{(__bridge id)kAXTrustedCheckOptionPrompt:@YES});}
@end
