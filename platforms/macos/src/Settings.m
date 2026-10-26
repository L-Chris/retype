#import "App.h"
#import "Lucide.h"
#import <objc/runtime.h>
#import <ApplicationServices/ApplicationServices.h>
#import <QuartzCore/QuartzCore.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>

static NSColor *RTInk(void) { return [NSColor colorWithCalibratedRed:20/255. green:43/255. blue:59/255. alpha:1]; }
static NSColor *RTAccent(void) { return [NSColor colorWithCalibratedRed:19/255. green:143/255. blue:150/255. alpha:1]; }
static NSColor *RTAccentHover(void) { return [NSColor colorWithCalibratedRed:15/255. green:122/255. blue:128/255. alpha:1]; }
static NSColor *RTAccentSoft(void) { return [NSColor colorWithCalibratedRed:232/255. green:247/255. blue:248/255. alpha:1]; }
static NSColor *RTMuted(void) { return [NSColor colorWithCalibratedRed:101/255. green:119/255. blue:129/255. alpha:1]; }
static NSColor *RTBorder(void) { return [NSColor colorWithCalibratedRed:222/255. green:230/255. blue:235/255. alpha:1]; }
static NSColor *RTDividerColor(void) { return [NSColor colorWithCalibratedRed:238/255. green:243/255. blue:246/255. alpha:1]; }
static NSColor *RTDanger(void) { return [NSColor colorWithCalibratedRed:206/255. green:62/255. blue:62/255. alpha:1]; }
static NSColor *RTDangerSoft(void) { return [NSColor colorWithCalibratedRed:253/255. green:242/255. blue:242/255. alpha:1]; }
static NSColor *RTOnline(void) { return [NSColor colorWithCalibratedRed:22/255. green:163/255. blue:106/255. alpha:1]; }

static NSTextField *RTLabel(NSString *text, CGFloat size) {
    NSTextField *label = [NSTextField labelWithString:text ?: @""];
    label.font = [NSFont systemFontOfSize:size]; label.textColor = RTInk(); label.selectable = NO;
    return label;
}
static NSTextField *RTBoldLabel(NSString *text, CGFloat size) {
    NSTextField *label = RTLabel(text, size);
    label.font = [NSFont systemFontOfSize:size weight:NSFontWeightMedium];
    return label;
}
static NSImage *RTLogoImage(void) {
    NSImage *image = [NSImage imageNamed:@"logo"];
    if (image) return image;
    NSString *bundled = [NSBundle.mainBundle pathForResource:@"logo" ofType:@"png"];
    if (bundled) return [[NSImage alloc] initWithContentsOfFile:bundled];
    NSString *cwd = [NSFileManager.defaultManager.currentDirectoryPath stringByAppendingPathComponent:@"assets/logo.png"];
    if ([NSFileManager.defaultManager fileExistsAtPath:cwd]) return [[NSImage alloc] initWithContentsOfFile:cwd];
    return nil;
}
static NSImage *RTSymbolImage(NSString *name, CGFloat pointSize, NSFontWeight weight, NSColor *tint) {
    if (!name.length) return nil;
    NSImage *shared = RTLucideImage(name, pointSize, tint);
    if (shared) return shared;
    NSImage *base = [NSImage imageWithSystemSymbolName:name accessibilityDescription:nil];
    if (!base) return nil;
    NSImageSymbolConfiguration *cfg = [NSImageSymbolConfiguration configurationWithPointSize:pointSize weight:weight];
    NSImage *configured = [base imageWithSymbolConfiguration:cfg] ?: base;
    if (!tint) return configured;
    NSImage *tinted = [configured copy];
    [tinted lockFocus];
    [tint set];
    NSRectFillUsingOperation(NSMakeRect(0, 0, tinted.size.width, tinted.size.height), NSCompositingOperationSourceAtop);
    [tinted unlockFocus];
    tinted.template = NO;
    return tinted;
}

// Metadata only: no field values, provider URLs, credentials or input text.
static void RTWriteDiagnosticLog(NSString *filename, NSDictionary *entry) {
    static dispatch_queue_t queue;static dispatch_once_t once;
    dispatch_once(&once,^{queue=dispatch_queue_create("retype.settings.diagnostics",DISPATCH_QUEUE_SERIAL);});
    dispatch_async(queue,^{
        NSString *directory=[RTDataRoot() stringByAppendingPathComponent:@"logs"];
        NSFileManager *files=NSFileManager.defaultManager;
        [files createDirectoryAtPath:directory withIntermediateDirectories:YES attributes:@{NSFilePosixPermissions:@0700} error:NULL];
        NSString *path=[directory stringByAppendingPathComponent:filename];
        if([[files attributesOfItemAtPath:path error:NULL][NSFileSize] unsignedLongLongValue]>=262144){
            NSString *previous=[path stringByAppendingString:@".previous"];
            [files removeItemAtPath:previous error:NULL];[files moveItemAtPath:path toPath:previous error:NULL];
        }
        if(![files fileExistsAtPath:path])[files createFileAtPath:path contents:nil attributes:@{NSFilePosixPermissions:@0600}];
        NSFileHandle *handle=[NSFileHandle fileHandleForWritingAtPath:path];
        @try{[handle seekToEndOfFile];[handle writeData:[NSJSONSerialization dataWithJSONObject:entry options:0 error:NULL]];[handle writeData:[@"\n" dataUsingEncoding:NSUTF8StringEncoding]];}@catch(NSException *e){(void)e;}@finally{[handle closeFile];}
    });
}
static void RTWriteSettingsLog(NSDictionary *entry) {RTWriteDiagnosticLog(@"settings-performance.jsonl",entry);}
static void RTSettingsLog(NSString *event, NSInteger page, CFAbsoluteTime started, BOOL success) {
    double elapsed=MAX(0,(CFAbsoluteTimeGetCurrent()-started)*1000);
    RTWriteSettingsLog(@{@"time":@(NSDate.date.timeIntervalSince1970),@"event":event,@"page":@(page),@"duration_ms":@(elapsed),@"success":@(success)});
}
void RTSettingsOpenLog(NSString *stage, NSString *source, BOOL visible) {
    RTWriteSettingsLog(@{@"time":@(NSDate.date.timeIntervalSince1970),@"event":@"settings_open",@"stage":stage,@"source":source,@"visible":@(visible),@"pid":@(NSProcessInfo.processInfo.processIdentifier),@"version":NSBundle.mainBundle.infoDictionary[@"CFBundleShortVersionString"]?:@"test"});
}
void RTUpdateLog(NSString *stage, BOOL success) {
    RTWriteDiagnosticLog(@"updates.jsonl",@{@"time":@(NSDate.date.timeIntervalSince1970),@"stage":stage,@"success":@(success),@"pid":@(NSProcessInfo.processInfo.processIdentifier),@"version":NSBundle.mainBundle.infoDictionary[@"CFBundleShortVersionString"]?:@"test"});
}
void RTUpdateSaveLog(BOOL saved, NSString *reason) {
    // Flush errors describe the worker/bridge state, never input content.
    NSMutableDictionary *entry=[@{@"time":@(NSDate.date.timeIntervalSince1970),@"stage":saved?@"saved_before_exit":@"save_failed",@"success":@(saved),@"pid":@(NSProcessInfo.processInfo.processIdentifier),@"version":NSBundle.mainBundle.infoDictionary[@"CFBundleShortVersionString"]?:@"test"} mutableCopy];
    if(!saved)entry[@"reason"]=reason?:@"保存结果无效";
    RTWriteDiagnosticLog(@"updates.jsonl",entry);
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

@interface RTNavButton : NSButton
@property(nonatomic, copy) NSString *symbolName;
@property(nonatomic) BOOL activeItem;
@property(nonatomic) BOOL hovered;
@property(nonatomic, strong) NSTrackingArea *trackingArea;
@end
@implementation RTNavButton
- (BOOL)isFlipped { return YES; }
- (void)updateTrackingAreas {
    [super updateTrackingAreas];
    if (self.trackingArea) [self removeTrackingArea:self.trackingArea];
    self.trackingArea = [[NSTrackingArea alloc] initWithRect:NSZeroRect
        options:NSTrackingMouseEnteredAndExited | NSTrackingActiveInKeyWindow | NSTrackingInVisibleRect
        owner:self userInfo:nil];
    [self addTrackingArea:self.trackingArea];
}
- (void)mouseEntered:(NSEvent *)event { (void)event; self.hovered = YES; self.needsDisplay = YES; }
- (void)mouseExited:(NSEvent *)event { (void)event; self.hovered = NO; self.needsDisplay = YES; }
- (void)setActiveItem:(BOOL)activeItem {
    if (_activeItem == activeItem) return;
    _activeItem = activeItem;
    self.needsDisplay = YES;
}
- (void)drawRect:(NSRect)dirtyRect {
    (void)dirtyRect;
    if (self.activeItem) {
        [NSColor.whiteColor setFill];
        [[NSBezierPath bezierPathWithRoundedRect:self.bounds xRadius:10 yRadius:10] fill];
        [RTAccent() setFill];
        [[NSBezierPath bezierPathWithRoundedRect:NSMakeRect(4, NSMidY(self.bounds) - 8, 3, 16) xRadius:1.5 yRadius:1.5] fill];
    } else if (self.hovered) {
        [[NSColor colorWithCalibratedWhite:1.0 alpha:0.55] setFill];
        [[NSBezierPath bezierPathWithRoundedRect:self.bounds xRadius:10 yRadius:10] fill];
    }
    NSColor *color = self.activeItem ? RTAccent() : RTInk();
    // Shared 24-unit Lucide geometry uses a fixed 20pt navigation size in all states.
    NSImage *icon = RTSymbolImage(self.symbolName, 20.0, NSFontWeightMedium, color);
    if (icon) {
        NSSize iconSize = icon.size;
        NSRect iconRect = NSMakeRect(round(21.0 - iconSize.width / 2.0), round(NSMidY(self.bounds) - iconSize.height / 2.0), iconSize.width, iconSize.height);
        [icon drawInRect:iconRect fromRect:NSZeroRect operation:NSCompositingOperationSourceOver fraction:1.0 respectFlipped:YES hints:nil];
    }
    NSFont *font = [NSFont systemFontOfSize:14 weight:self.activeItem ? NSFontWeightSemibold : NSFontWeightRegular];
    NSDictionary *attrs = @{NSFontAttributeName: font, NSForegroundColorAttributeName: color};
    CGFloat lineHeight = ceil(font.ascender - font.descender);
    NSPoint textPt = NSMakePoint(38.0, round((NSHeight(self.bounds) - lineHeight) / 2.0));
    [self.title drawAtPoint:textPt withAttributes:attrs];
}
@end

typedef NS_ENUM(NSInteger, RTButtonStyle) {
    RTButtonStyleSecondary = 0,
    RTButtonStylePrimary = 1,
    RTButtonStyleDanger = 2,
};

@interface RTButton : NSButton
@property(nonatomic) RTButtonStyle buttonStyle;
@property(nonatomic, copy) NSString *symbolName;
@property(nonatomic) BOOL hovered;
@property(nonatomic, strong) NSTrackingArea *trackingArea;
@end
@implementation RTButton
- (instancetype)initWithFrame:(NSRect)frameRect {
    if ((self = [super initWithFrame:frameRect])) {
        self.title = @"";
    }
    return self;
}
- (BOOL)isFlipped { return YES; }
- (void)updateTrackingAreas {
    [super updateTrackingAreas];
    if (self.trackingArea) [self removeTrackingArea:self.trackingArea];
    self.trackingArea = [[NSTrackingArea alloc] initWithRect:NSZeroRect
        options:NSTrackingMouseEnteredAndExited | NSTrackingActiveInKeyWindow | NSTrackingInVisibleRect
        owner:self userInfo:nil];
    [self addTrackingArea:self.trackingArea];
}
- (void)mouseEntered:(NSEvent *)event { (void)event; self.hovered = YES; self.needsDisplay = YES; }
- (void)mouseExited:(NSEvent *)event { (void)event; self.hovered = NO; self.needsDisplay = YES; }
- (void)drawRect:(NSRect)dirtyRect {
    (void)dirtyRect;
    NSRect rect = NSInsetRect(self.bounds, 0.5, 0.5);
    NSBezierPath *path = [NSBezierPath bezierPathWithRoundedRect:rect xRadius:8 yRadius:8];
    CGFloat alpha = self.enabled ? 1.0 : 0.45;
    NSColor *fill = nil, *stroke = nil, *text = nil;
    if (self.buttonStyle == RTButtonStylePrimary) {
        fill = [(self.hovered && self.enabled ? RTAccentHover() : RTAccent()) colorWithAlphaComponent:alpha];
        text = [NSColor.whiteColor colorWithAlphaComponent:alpha];
    } else if (self.buttonStyle == RTButtonStyleDanger) {
        fill = [(self.hovered && self.enabled ? RTDangerSoft() : NSColor.whiteColor) colorWithAlphaComponent:alpha];
        stroke = [[NSColor colorWithCalibratedRed:238/255. green:205/255. blue:205/255. alpha:1] colorWithAlphaComponent:alpha];
        text = [RTDanger() colorWithAlphaComponent:alpha];
    } else {
        fill = [(self.hovered && self.enabled ? RTAccentSoft() : NSColor.whiteColor) colorWithAlphaComponent:alpha];
        stroke = [(self.hovered && self.enabled ? RTAccent() : RTBorder()) colorWithAlphaComponent:alpha];
        text = [(self.hovered && self.enabled ? RTAccent() : RTInk()) colorWithAlphaComponent:alpha];
    }
    [fill setFill]; [path fill];
    if (stroke) { path.lineWidth = 1; [stroke setStroke]; [path stroke]; }

    NSImage *icon = RTSymbolImage(self.symbolName, 18.0, NSFontWeightMedium, text);
    BOOL hasTitle = self.title.length > 0;
    if (icon && !hasTitle) {
        NSSize iconSize = icon.size;
        NSRect iconRect = NSMakeRect(round(NSMidX(self.bounds) - iconSize.width / 2.0), round(NSMidY(self.bounds) - iconSize.height / 2.0), iconSize.width, iconSize.height);
        [icon drawInRect:iconRect fromRect:NSZeroRect operation:NSCompositingOperationSourceOver fraction:1.0 respectFlipped:YES hints:nil];
        return;
    }
    NSFont *font = [NSFont systemFontOfSize:13 weight:self.buttonStyle == RTButtonStylePrimary ? NSFontWeightMedium : NSFontWeightRegular];
    NSDictionary *attrs = @{NSFontAttributeName: font, NSForegroundColorAttributeName: text};
    NSSize titleSize = [self.title sizeWithAttributes:attrs];
    if (icon && hasTitle) {
        NSSize iconSize = icon.size;
        CGFloat totalWidth = iconSize.width + 5.0 + titleSize.width;
        CGFloat startX = round(NSMidX(self.bounds) - totalWidth / 2.0);
        NSRect iconRect = NSMakeRect(startX, round(NSMidY(self.bounds) - iconSize.height / 2.0), iconSize.width, iconSize.height);
        [icon drawInRect:iconRect fromRect:NSZeroRect operation:NSCompositingOperationSourceOver fraction:1.0 respectFlipped:YES hints:nil];
        [self.title drawAtPoint:NSMakePoint(startX + iconSize.width + 5.0, round(NSMidY(self.bounds) - titleSize.height / 2.0)) withAttributes:attrs];
    } else {
        NSPoint pt = NSMakePoint(round(NSMidX(self.bounds) - titleSize.width / 2.0), round(NSMidY(self.bounds) - titleSize.height / 2.0));
        [self.title drawAtPoint:pt withAttributes:attrs];
    }
}
@end

@interface RTLightScroller : NSScroller @end
@implementation RTLightScroller
+ (CGFloat)scrollerWidthForControlSize:(NSControlSize)size scrollerStyle:(NSScrollerStyle)style { (void)size; (void)style; return 8; }
- (void)drawKnob {
    [[NSColor colorWithCalibratedWhite:0.78 alpha:0.65] setFill];
    NSRect rect = NSInsetRect([self rectForPart:NSScrollerKnob], 2, 1);
    [[NSBezierPath bezierPathWithRoundedRect:rect xRadius:2 yRadius:2] fill];
}
- (void)drawKnobSlotInRect:(NSRect)rect highlight:(BOOL)highlight { (void)rect; (void)highlight; }
@end

static NSRect RTVerticalCenterRect(NSRect bounds, NSRect inner, NSFont *font) {
    NSFont *f = font ?: [NSFont systemFontOfSize:14];
    CGFloat lineHeight = MAX(ceil(f.ascender - f.descender + f.leading) + 2.0, 20.0);
    if (NSHeight(bounds) <= lineHeight + 4) return inner;
    CGFloat y = NSMinY(bounds) + floor((NSHeight(bounds) - lineHeight) / 2.0);
    return NSMakeRect(NSMinX(inner), y, NSWidth(inner), lineHeight);
}

@interface RTVerticalCell : NSTextFieldCell @end
@implementation RTVerticalCell
- (NSRect)drawingRectForBounds:(NSRect)rect {
    return RTVerticalCenterRect(rect, [super drawingRectForBounds:rect], self.font);
}
@end

@interface RTVerticalSecureCell : NSSecureTextFieldCell @end
@implementation RTVerticalSecureCell
- (NSRect)drawingRectForBounds:(NSRect)rect {
    return RTVerticalCenterRect(rect, [super drawingRectForBounds:rect], self.font);
}
@end

@interface RTShortcutButton : NSButton
@property(nonatomic, copy) void (^captured)(NSDictionary *);
@property(nonatomic) BOOL capturing;
@property(nonatomic) unsigned short modifierKey;
@end
@implementation RTShortcutButton
- (BOOL)isFlipped { return YES; }
- (BOOL)acceptsFirstResponder { return YES; }
- (void)mouseDown:(NSEvent *)event {
    (void)event; self.capturing = YES; self.title = @"请按快捷键…"; self.needsDisplay = YES; [self.window makeFirstResponder:self];
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
- (void)drawRect:(NSRect)dirtyRect {
    (void)dirtyRect;
    NSRect rect = NSInsetRect(self.bounds, 0.5, 0.5);
    NSBezierPath *path = [NSBezierPath bezierPathWithRoundedRect:rect xRadius:8 yRadius:8];
    if (self.capturing) {
        [RTAccentSoft() setFill]; [path fill];
        [RTAccent() setStroke]; path.lineWidth = 1.5; [path stroke];
    } else {
        [[NSColor colorWithCalibratedRed:248/255. green:250/255. blue:251/255. alpha:1] setFill]; [path fill];
        [RTBorder() setStroke]; path.lineWidth = 1; [path stroke];
    }
    NSColor *color = self.capturing ? RTAccent() : RTInk();
    NSFont *font = [NSFont systemFontOfSize:13 weight:NSFontWeightMedium];
    NSDictionary *attrs = @{NSFontAttributeName: font, NSForegroundColorAttributeName: color};
    NSSize size = [self.title sizeWithAttributes:attrs];
    [self.title drawAtPoint:NSMakePoint(NSMidX(self.bounds) - size.width / 2, NSMidY(self.bounds) - size.height / 2) withAttributes:attrs];
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

static NSString *RTFormatCode(NSString *code) {
    if (code.length != 6) return code ?: @"";
    NSMutableArray<NSString *> *parts = NSMutableArray.array;
    for (NSUInteger i = 0; i < 6; i++) {
        [parts addObject:[code substringWithRange:NSMakeRange(i, 1)]];
        if (i == 2) [parts addObject:@" "];
    }
    return [parts componentsJoinedByString:@" "];
}

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
@property(nonatomic) BOOL lanJoining;
@property(nonatomic, copy) NSString *lanJoinCode;
@property(nonatomic, copy) NSString *lanJoinAddress;
@property(nonatomic, copy) NSString *editingPeerID;
@property(nonatomic, copy) NSString *editingPeerAddress;
@property(nonatomic) BOOL editingLanPort;
@property(nonatomic, copy) NSString *lanPortValue;
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
        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,800,600)
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
    NSView *sidebar = [[NSView alloc] initWithFrame:NSMakeRect(0,0,190,600)]; sidebar.wantsLayer = YES;
    sidebar.layer.backgroundColor = [NSColor colorWithCalibratedRed:222/255. green:246/255. blue:248/255. alpha:1].CGColor;
    [root addSubview:sidebar];
    NSView *sidebarBorder = [[NSView alloc] initWithFrame:NSMakeRect(189,0,1,600)]; sidebarBorder.wantsLayer = YES;
    sidebarBorder.layer.backgroundColor = [NSColor colorWithCalibratedRed:206/255. green:233/255. blue:236/255. alpha:1].CGColor;
    [sidebar addSubview:sidebarBorder];
    NSImageView *logo = [[NSImageView alloc] initWithFrame:NSMakeRect(20,528,34,34)];
    logo.image = RTLogoImage(); logo.wantsLayer = YES; logo.layer.cornerRadius = 8; logo.layer.masksToBounds = YES;
    [sidebar addSubview:logo];
    NSTextField *brand = RTBoldLabel(@"retype",16); brand.frame = NSMakeRect(62,534,118,22); [sidebar addSubview:brand];
    NSArray *titles = @[@"输入",@"词库",@"AI 提供商",@"翻译",@"语音输入",@"快捷键",@"统计",@"云同步",@"跨设备",@"关于"];
    NSArray *icons = @[@"keyboard",@"book-open",@"cpu",@"languages",@"mic",@"command",@"chart-no-axes-column",@"cloud",@"monitor-smartphone",@"info"];
    self.navigation = [[NSStackView alloc] initWithFrame:NSMakeRect(12,58,166,440)];
    self.navigation.orientation = NSUserInterfaceLayoutOrientationVertical; self.navigation.spacing = 4; self.navigation.distribution = NSStackViewDistributionFillEqually;
    self.navigation.alignment = NSLayoutAttributeLeading;
    [sidebar addSubview:self.navigation];
    for (NSUInteger i = 0; i < titles.count; i++) {
        RTNavButton *button = [RTNavButton new]; button.cell = [RTNavigationCell new];
        button.title = titles[i];
        NSString *sym = icons[i];
        if (!RTLucideImage(sym, 20, RTInk()) && ![NSImage imageWithSystemSymbolName:sym accessibilityDescription:nil] && i == 8) sym = @"doc.on.clipboard";
        button.symbolName = sym;
        button.image = [NSImage imageWithSystemSymbolName:sym accessibilityDescription:titles[i]];
        button.target = self; button.action = @selector(navigate:);
        button.tag = (NSInteger)i; button.bordered = NO; button.alignment = NSTextAlignmentLeft; button.imagePosition = NSImageLeft;
        button.font = [NSFont systemFontOfSize:14];
        [self.navigation addArrangedSubview:button];
        button.translatesAutoresizingMaskIntoConstraints = NO;
        [button.widthAnchor constraintEqualToAnchor:self.navigation.widthAnchor].active = YES;
    }
    self.titleLabel = RTBoldLabel(@"输入",24); self.titleLabel.frame = NSMakeRect(222,525,556,36); [root addSubview:self.titleLabel];
    self.scroll = [[NSScrollView alloc] initWithFrame:NSMakeRect(212,36,580,472)];
    self.scroll.hasVerticalScroller = YES; self.scroll.autohidesScrollers = YES; self.scroll.scrollerStyle = NSScrollerStyleOverlay;
    self.scroll.verticalScroller = [RTLightScroller new]; self.scroll.drawsBackground = NO;
    self.body = [[RTFlipped alloc] initWithFrame:NSMakeRect(0,0,580,472)]; self.scroll.documentView = self.body; [root addSubview:self.scroll];
    self.messageLabel = RTLabel(@"",12); self.messageLabel.textColor = RTMuted(); self.messageLabel.frame = NSMakeRect(224,4,554,28);
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
- (NSString *)secretAccountForPath:(NSString *)path {
    if ([path isEqual:@"secret:cloud"]) return [@"sync/" stringByAppendingString:[self cloudAccount]];
    return [path substringFromIndex:7];
}
- (id)read:(NSString *)path {
    if ([path hasPrefix:@"secret:"]) return self.secretValues[[self secretAccountForPath:path]];
    if ([path isEqual:@"testModel"]) return self.testModel ?: [[self provider][@"models"] firstObject];
    if ([path isEqual:@"lan.joinCode"]) return self.lanJoinCode ?: @"";
    if ([path isEqual:@"lan.joinAddress"]) return self.lanJoinAddress ?: @"";
    if ([path isEqual:@"lan.peerAddress"]) return self.editingPeerAddress ?: @"";
    if ([path isEqual:@"lan.port"]) return self.lanPortValue ?: @"50206";
    if ([path hasPrefix:@"provider."]) return [[self provider] valueForKeyPath:[path substringFromIndex:9]];
    return [RTPreferences.shared.value valueForKeyPath:path];
}
- (void)write:(NSString *)path value:(id)value {
    if ([path isEqual:@"lan.joinCode"]) { self.lanJoinCode = value; return; }
    if ([path isEqual:@"lan.joinAddress"]) { self.lanJoinAddress = value; return; }
    if ([path isEqual:@"lan.peerAddress"]) { self.editingPeerAddress = value; return; }
    if ([path isEqual:@"lan.port"]) { self.lanPortValue = value; return; }
    if ([path hasPrefix:@"secret:"]) {
        NSString *account=[self secretAccountForPath:path],*text=[value copy];
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
    card.layer.borderWidth = 1; card.layer.borderColor = RTBorder().CGColor;
    [self.body addSubview:card]; self.card = card; self.row = 16; return card;
}
- (void)finishCard {
    NSRect frame = self.card.frame; frame.size.height = self.row + 10; self.card.frame = frame;
    self.y += frame.size.height + 14;
}
- (void)divider {
    NSView *line = [[NSView alloc] initWithFrame:NSMakeRect(20, self.row, 516, 1)];
    line.wantsLayer = YES; line.layer.backgroundColor = RTDividerColor().CGColor;
    [self.card addSubview:line]; self.row += 13;
}
- (void)label:(NSString *)text size:(CGFloat)size {
    NSTextField *label = size >= 15 ? RTBoldLabel(text, size) : RTLabel(text, size);
    if (size <= 12) label.textColor = RTMuted();
    label.frame = NSMakeRect(20,self.row,516,size + 10); [self.card addSubview:label]; self.row += size + 16;
}
- (NSTextField *)rowLabel:(NSString *)title {
    NSTextField *label = RTLabel(title,14); label.frame = NSMakeRect(20,self.row+7,112,22); [self.card addSubview:label]; return label;
}
- (void)loadSecret:(NSString *)path field:(NSSecureTextField *)field {
    NSString *account=[self secretAccountForPath:path];id cached=self.secretValues[account];
    if(cached){RTUpdateSecretFields(field,path,[cached isKindOfClass:NSString.class]?cached:nil);return;}
    field.enabled=NO;field.placeholderString=@"正在读取钥匙串…";
    if([self.secretLoading containsObject:account])return;
    [self.secretLoading addObject:account];NSUInteger version=[self.secretVersions[account] unsignedIntegerValue];NSInteger page=self.page;
    dispatch_async(self.secretQueue,^{
        CFAbsoluteTime started=CFAbsoluteTimeGetCurrent();NSString *value=RTSecret(account);
        if(!value.length && [account hasPrefix:@"sync/"]) value=RTSecret(@"sync/") ?: value;
        RTSettingsLog(@"keychain_read",page,started,value!=nil);
        dispatch_async(dispatch_get_main_queue(),^{
            [self.secretLoading removeObject:account];
            if([self.secretVersions[account] unsignedIntegerValue]!=version)return;
            self.secretValues[account]=value?:NSNull.null;
            RTUpdateSecretFields(self.body,path,value);
        });
    });
}
- (NSTextField *)textFieldAtX:(CGFloat)x width:(CGFloat)width path:(NSString *)path placeholder:(NSString *)placeholder secure:(BOOL)secure {
    NSTextField *field = secure ? [[NSSecureTextField alloc] initWithFrame:NSMakeRect(x,self.row,width,34)] : [[NSTextField alloc] initWithFrame:NSMakeRect(x,self.row,width,34)];
    field.cell = secure ? [RTVerticalSecureCell new] : [RTVerticalCell new];
    field.font = [NSFont systemFontOfSize:14]; field.identifier = path; field.delegate = self;
    field.controlSize = NSControlSizeLarge;
    field.editable=YES;field.selectable=YES;field.bezeled=YES;field.cell.usesSingleLineMode=YES;field.cell.scrollable=YES;
    field.bezelStyle = NSTextFieldRoundedBezel; field.drawsBackground = YES; field.backgroundColor = NSColor.whiteColor;
    if (placeholder.length) field.placeholderString = placeholder;
    if(secure)[self loadSecret:path field:(NSSecureTextField *)field];
    else{ id value=[self read:path];field.stringValue=[value isKindOfClass:NSString.class]?value:[value description]?:@""; }
    [self.card addSubview:field]; return field;
}
- (NSTextField *)field:(NSString *)title path:(NSString *)path secure:(BOOL)secure {
    [self rowLabel:title];
    NSTextField *field = [self textFieldAtX:140 width:392 path:path placeholder:nil secure:secure];
    self.row += 46; return field;
}
- (NSPopUpButton *)popup:(NSString *)title path:(NSString *)path labels:(NSArray<NSString *> *)labels values:(NSArray *)values action:(SEL)action {
    [self rowLabel:title]; NSPopUpButton *popup = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(140,self.row,392,34) pullsDown:NO];
    popup.identifier = path; popup.font = [NSFont systemFontOfSize:14]; popup.target = self; popup.action = action ?: @selector(select:);
    popup.controlSize = NSControlSizeLarge;
    for (NSUInteger i = 0; i < labels.count; i++) { [popup addItemWithTitle:labels[i]]; popup.lastItem.representedObject = values[i]; if ([[self read:path] isEqual:values[i]]) [popup selectItem:popup.lastItem]; }
    [self.card addSubview:popup]; self.row += 46; return popup;
}
- (NSSwitch *)toggle:(NSString *)title path:(NSString *)path {
    NSTextField *label=[self rowLabel:title];[label setFrameSize:NSMakeSize(440,22)];
    NSSwitch *toggle = [[NSSwitch alloc] initWithFrame:NSMakeRect(488,self.row+3,48,28)];
    toggle.identifier = path; toggle.state = [[self read:path] boolValue] ? NSControlStateValueOn : NSControlStateValueOff;
    toggle.target = self; toggle.action = @selector(toggle:); [self.card addSubview:toggle]; self.row += 42;
    return toggle;
}
- (RTButton *)styledButton:(NSString *)title symbol:(NSString *)symbol tooltip:(NSString *)tooltip style:(RTButtonStyle)style action:(SEL)action x:(CGFloat)x width:(CGFloat)width {
    RTButton *button = [[RTButton alloc] initWithFrame:NSMakeRect(x,self.row,width,34)];
    button.title = title ?: @""; button.symbolName = symbol; button.buttonStyle = style;
    if (tooltip.length) { button.toolTip = tooltip; button.accessibilityLabel = tooltip; }
    button.target = self; button.action = action;
    [self.card addSubview:button]; return button;
}
- (RTButton *)styledButton:(NSString *)title style:(RTButtonStyle)style action:(SEL)action x:(CGFloat)x width:(CGFloat)width {
    return [self styledButton:title symbol:nil tooltip:nil style:style action:action x:x width:width];
}
- (RTButton *)iconButton:(NSString *)symbol tooltip:(NSString *)tooltip style:(RTButtonStyle)style action:(SEL)action x:(CGFloat)x {
    return [self styledButton:@"" symbol:symbol tooltip:tooltip style:style action:action x:x width:34];
}
- (NSButton *)button:(NSString *)title action:(SEL)action x:(CGFloat)x width:(CGFloat)width {
    return [self styledButton:title style:RTButtonStyleSecondary action:action x:x width:width];
}
- (void)modelRow:(BOOL)voice {
    NSString *providerPath = voice ? @"ai.voice.provider" : @"ai.provider", *modelPath = voice ? @"ai.voice.model" : @"ai.model";
    NSMutableArray *labels = NSMutableArray.array, *values = NSMutableArray.array;
    for (NSDictionary *p in RTPreferences.shared.value[@"ai"][@"providers"]) {
        if (voice && ![@[@"Compatible",@"Gemini"] containsObject:p[@"kind"]]) continue;
        [labels addObject:p[@"name"]]; [values addObject:p[@"id"]];
    }
    if (!labels.count) { [self label:@"请先在「AI 提供商」中添加服务。" size:13]; return; }
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

- (void)renderLanPage:(RTAppDelegate *)app {
    NSDictionary *lan = app.syncStatus[@"lan"] ?: @{};
    NSDictionary *config = lan[@"config"] ?: @{};
    NSDictionary *status = lan[@"status"] ?: @{};
    BOOL enabled = [config[@"enabled"] boolValue];
    NSArray *peers = config[@"peers"] ?: @[];
    NSArray *online = status[@"online"] ?: @[];
    NSDictionary *connections = status[@"connections"] ?: @{};
    NSArray *addresses = status[@"addresses"] ?: @[];
    NSString *code = status[@"code"] ?: @"";
    NSString *statusMsg = status[@"message"] ?: @"";

    // Card 1: Master Switch & Pairing
    [self newCard];
    NSTextField *title = RTBoldLabel(@"剪贴板同步", 15);
    title.frame = NSMakeRect(20, self.row + 5, 200, 22); [self.card addSubview:title];
    NSSwitch *toggle = [[NSSwitch alloc] initWithFrame:NSMakeRect(488, self.row + 2, 48, 28)];
    toggle.target = self; toggle.action = @selector(lanEnable:);
    toggle.state = enabled ? NSControlStateValueOn : NSControlStateValueOff;
    [self.card addSubview:toggle];
    self.row += 40;

    if (enabled) {
        [self divider];
        NSView *dot = [[NSView alloc] initWithFrame:NSMakeRect(20, self.row + 13, 8, 8)];
        dot.wantsLayer = YES; dot.layer.cornerRadius = 4;
        dot.layer.backgroundColor = (online.count > 0 ? RTOnline() : RTAccent()).CGColor;
        [self.card addSubview:dot];

        NSString *summary = statusMsg.length ? statusMsg : (online.count > 0 ? [NSString stringWithFormat:@"已连接 %lu 台设备", (unsigned long)online.count] : @"等待设备连接");
        NSTextField *stateLabel = RTLabel(summary, 13);
        stateLabel.textColor = RTMuted();
        stateLabel.frame = NSMakeRect(34, self.row + 8, 280, 20);
        [self.card addSubview:stateLabel];

        [self styledButton:@"匹配码" symbol:@"number.square" tooltip:@"生成 6 位配对匹配码" style:code.length ? RTButtonStylePrimary : RTButtonStyleSecondary action:@selector(pairCode:) x:336 width:94];
        [self styledButton:@"关联" symbol:@"link.badge.plus" tooltip:@"输入匹配码关联其他设备" style:self.lanJoining ? RTButtonStylePrimary : RTButtonStyleSecondary action:@selector(joinDevice:) x:438 width:98];
        self.row += 44;

        if (code.length) {
            RTFlipped *box = [[RTFlipped alloc] initWithFrame:NSMakeRect(20, self.row, 516, 56)];
            box.wantsLayer = YES; box.layer.backgroundColor = RTAccentSoft().CGColor; box.layer.cornerRadius = 10;
            [self.card addSubview:box];

            NSTextField *codeLabel = [NSTextField labelWithString:RTFormatCode(code)];
            codeLabel.font = [NSFont monospacedDigitSystemFontOfSize:24 weight:NSFontWeightBold];
            codeLabel.textColor = RTAccent(); codeLabel.selectable = NO;
            codeLabel.frame = NSMakeRect(18, 12, 180, 32);
            [box addSubview:codeLabel];

            long long expires = [status[@"expires"] longLongValue];
            long long nowMs = (long long)(NSDate.date.timeIntervalSince1970 * 1000);
            NSInteger remain = (NSInteger)MAX(0LL, (expires - nowMs) / 1000);
            if (remain > 0) {
                NSTextField *hint = RTLabel([NSString stringWithFormat:@"%lds 后失效", (long)remain], 12);
                hint.textColor = RTMuted(); hint.frame = NSMakeRect(204, 19, 140, 18);
                [box addSubview:hint];
            }

            RTButton *cancel = [[RTButton alloc] initWithFrame:NSMakeRect(470, 11, 34, 34)];
            cancel.symbolName = @"x"; cancel.toolTip = @"关闭匹配码"; cancel.accessibilityLabel = @"关闭匹配码";
            cancel.target = self; cancel.action = @selector(cancelPair:);
            [box addSubview:cancel];
            self.row += 64;
        }

        if (self.lanJoining) {
            [self divider];
            [self rowLabel:@"匹配码"];
            [self textFieldAtX:140 width:312 path:@"lan.joinCode" placeholder:@"输入 6 位数字" secure:NO];
            [self iconButton:@"check" tooltip:@"确认关联" style:RTButtonStylePrimary action:@selector(submitJoin:) x:460];
            [self iconButton:@"x" tooltip:@"取消" style:RTButtonStyleSecondary action:@selector(cancelJoin:) x:502];
            self.row += 44;
            [self rowLabel:@"指定地址"];
            [self textFieldAtX:140 width:396 path:@"lan.joinAddress" placeholder:@"选填，如 100.64.0.1:50206（局域网留空）" secure:NO];
            self.row += 42;
        }
    }
    [self finishCard];

    // Card 2: Connected / Associated Peers
    if (enabled || peers.count > 0) {
        [self newCard];
        NSTextField *peersHeader = RTBoldLabel(@"已关联设备", 13);
        peersHeader.textColor = RTMuted();
        peersHeader.frame = NSMakeRect(20, self.row, 200, 20);
        [self.card addSubview:peersHeader];
        self.row += 28;

        if (!peers.count) {
            NSTextField *empty = RTLabel(@"暂无关联设备", 13);
            empty.textColor = RTMuted();
            empty.frame = NSMakeRect(20, self.row, 300, 22);
            [self.card addSubview:empty];
            self.row += 28;
        } else {
            for (NSUInteger i = 0; i < peers.count; i++) {
                NSDictionary *peer = peers[i];
                NSString *peerId = peer[@"id"] ?: @"";
                [self divider];

                BOOL isOnline = [online containsObject:peerId];
                NSString *connectedAddr = connections[peerId];
                NSArray *savedAddrs = peer[@"addresses"] ?: @[];
                NSString *addrText = connectedAddr.length ? connectedAddr : (savedAddrs.count ? [savedAddrs componentsJoinedByString:@", "] : @"自动发现");

                NSView *peerDot = [[NSView alloc] initWithFrame:NSMakeRect(20, self.row + 7, 8, 8)];
                peerDot.wantsLayer = YES; peerDot.layer.cornerRadius = 4;
                peerDot.layer.backgroundColor = (isOnline ? RTOnline() : [NSColor colorWithCalibratedWhite:0.75 alpha:1]).CGColor;
                [self.card addSubview:peerDot];

                NSTextField *nameLabel = RTBoldLabel(peer[@"name"] ?: @"设备", 14);
                [nameLabel sizeToFit];
                CGFloat nameWidth = MIN(260, NSWidth(nameLabel.frame));
                nameLabel.frame = NSMakeRect(34, self.row, nameWidth, 20);
                [self.card addSubview:nameLabel];

                NSTextField *badge = RTLabel(isOnline ? @"已连接" : @"离线", 12);
                badge.textColor = isOnline ? RTAccent() : RTMuted();
                badge.frame = NSMakeRect(34 + nameWidth + 8, self.row + 2, 64, 18);
                [self.card addSubview:badge];

                NSTextField *addrLabel = RTLabel(addrText, 12);
                addrLabel.font = [NSFont monospacedSystemFontOfSize:11.5 weight:NSFontWeightRegular];
                addrLabel.textColor = RTMuted();
                addrLabel.frame = NSMakeRect(34, self.row + 21, 410, 18);
                [self.card addSubview:addrLabel];

                RTButton *addrBtn = [self iconButton:@"sliders-horizontal" tooltip:@"连接地址" style:[self.editingPeerID isEqual:peerId] ? RTButtonStylePrimary : RTButtonStyleSecondary action:@selector(peerAddress:) x:460];
                addrBtn.identifier = peerId;
                RTButton *removeBtn = [self iconButton:@"trash" tooltip:@"解除关联" style:RTButtonStyleDanger action:@selector(removePeer:) x:502];
                removeBtn.identifier = peerId;
                self.row += 46;

                if ([self.editingPeerID isEqual:peerId]) {
                    [self textFieldAtX:34 width:418 path:@"lan.peerAddress" placeholder:@"IP 或域名:端口，多个用逗号分隔" secure:NO];
                    RTButton *saveAddr = [self iconButton:@"check" tooltip:@"保存地址" style:RTButtonStylePrimary action:@selector(savePeerAddress:) x:460];
                    saveAddr.identifier = peerId;
                    [self iconButton:@"x" tooltip:@"取消" style:RTButtonStyleSecondary action:@selector(cancelPeerAddress:) x:502];
                    self.row += 42;
                }
            }
        }
        [self finishCard];
    }

    // Card 3: Local Network & Port
    if (enabled) {
        [self newCard];
        [self rowLabel:@"监听端口"];
        NSNumber *portNum = config[@"port"];
        NSString *portText = portNum.unsignedIntegerValue ? portNum.stringValue : @"50206";
        if (self.editingLanPort) {
            [self textFieldAtX:140 width:120 path:@"lan.port" placeholder:@"50206" secure:NO];
            [self iconButton:@"check" tooltip:@"应用端口" style:RTButtonStylePrimary action:@selector(saveLanPort:) x:268];
            [self iconButton:@"x" tooltip:@"取消" style:RTButtonStyleSecondary action:@selector(cancelLanPort:) x:310];
        } else {
            NSTextField *portLabel = RTLabel(portText, 13);
            portLabel.font = [NSFont monospacedDigitSystemFontOfSize:13 weight:NSFontWeightRegular];
            portLabel.frame = NSMakeRect(140, self.row + 8, 90, 20);
            [self.card addSubview:portLabel];
            [self iconButton:@"pencil" tooltip:@"修改监听端口" style:RTButtonStyleSecondary action:@selector(lanPort:) x:502];
        }
        self.row += 42;

        if (addresses.count > 0) {
            [self divider];
            NSTextField *addrTitle = RTLabel(@"本机地址", 14);
            addrTitle.frame = NSMakeRect(20, self.row + 7, 112, 22);
            [self.card addSubview:addrTitle];
            for (NSString *address in addresses) {
                NSTextField *addrItem = RTLabel(address, 12);
                addrItem.font = [NSFont monospacedSystemFontOfSize:12 weight:NSFontWeightRegular];
                addrItem.textColor = RTMuted();
                addrItem.frame = NSMakeRect(140, self.row + 8, 350, 20);
                [self.card addSubview:addrItem];
                RTButton *copyBtn = [self iconButton:@"copy" tooltip:@"复制地址" style:RTButtonStyleSecondary action:@selector(copyAddress:) x:502];
                copyBtn.identifier = address;
                self.row += 38;
            }
        }
        [self finishCard];
    }
}

- (void)refresh {
    if (!self.body) return;
    CFAbsoluteTime started=CFAbsoluteTimeGetCurrent();
    RTAppDelegate *app = (RTAppDelegate *)NSApp.delegate;
    for (NSView *view in self.body.subviews.copy) [view removeFromSuperview];
    self.y = 0; self.card = nil;
    NSArray *titles = @[@"输入",@"词库",@"AI 提供商",@"翻译",@"语音输入",@"快捷键",@"统计",@"云同步",@"跨设备",@"关于"];
    self.titleLabel.stringValue = titles[self.page];
    for (RTNavButton *button in self.navigation.arrangedSubviews) {
        if ([button isKindOfClass:RTNavButton.class]) button.activeItem = (button.tag == self.page);
    }
    self.messageLabel.stringValue = app.messages[@(self.page)] ?: @"";

    if (self.page == 0) {
        [self newCard];
        [self popup:@"拼音方案" path:@"input.scheme" labels:@[@"全拼",@"小鹤双拼"] values:@[@0,@1] action:nil];
        [self finishCard];

        [self newCard];
        [self toggle:@"英文单词补全" path:@"input.english"];
        [self divider];
        NSSwitch *spelling = [self toggle:@"英文拼写建议" path:@"input.spelling"];
        spelling.enabled = [[self read:@"input.english"] boolValue];
        [self divider];
        [self toggle:@"符号自动补全" path:@"input.symbols"];
        [self finishCard];

        if (!AXIsProcessTrusted()) {
            [self newCard];
            NSTextField *axLabel = RTLabel(@"辅助功能权限", 14);
            axLabel.frame = NSMakeRect(20, self.row + 7, 260, 22); [self.card addSubview:axLabel];
            [self styledButton:@"授权" symbol:@"lock.open" tooltip:@"前往系统设置授予辅助功能权限" style:RTButtonStylePrimary action:@selector(accessibility:) x:448 width:88];
            self.row += 42;
            [self finishCard];
        }
    } else if (self.page == 1) {
        if (!app.runtime || app.resourceBusy) {
            [self newCard];
            NSTextField *baseTitle = RTBoldLabel(@"基础词库", 15);
            baseTitle.frame = NSMakeRect(20, self.row + 6, 220, 22); [self.card addSubview:baseTitle];
            if (app.resourceBusy) {
                [self iconButton:@"x" tooltip:@"取消下载" style:RTButtonStyleSecondary action:@selector(cancelResources:) x:502];
            } else {
                [self styledButton:@"下载" symbol:@"arrow.down.circle" tooltip:@"在线下载基础词库" style:RTButtonStylePrimary action:@selector(downloadResources:) x:344 width:92];
                [self styledButton:@"导入" symbol:@"folder" tooltip:@"从本地文件导入词库包" style:RTButtonStyleSecondary action:@selector(importResources:) x:444 width:92];
            }
            self.row += 42;
            if (app.resourceBusy) {
                self.progress = [[NSProgressIndicator alloc] initWithFrame:NSMakeRect(20,self.row,516,6)];
                self.progress.indeterminate = NO; self.progress.maxValue = 100;
                [self.card addSubview:self.progress]; self.row += 16;
            }
            if (app.resourceMessage.length) [self label:app.resourceMessage size:12];
            [self finishCard];
        }
        for (NSUInteger i = 0; i < self.packs.count; i++) {
            NSDictionary *p = self.packs[i]; [self newCard];
            NSTextField *packTitle = RTBoldLabel(p[@"title"], 15);
            packTitle.frame = NSMakeRect(20, 14, 420, 22); [self.card addSubview:packTitle];
            NSTextField *description = RTLabel([NSString stringWithFormat:@"%@ · %.1f MB",p[@"description"],[p[@"bytes"] doubleValue]/1048576],12);
            description.textColor = RTMuted(); description.frame = NSMakeRect(20, 38, 440, 20); [self.card addSubview:description];
            NSSwitch *toggle = [[NSSwitch alloc] initWithFrame:NSMakeRect(488, 22, 48, 28)]; toggle.tag = (NSInteger)i; toggle.target = self; toggle.action = @selector(pack:);
            toggle.state = ([RTPreferences.shared.value[@"packs"] unsignedIntegerValue] & (1 << i)) ? NSControlStateValueOn : NSControlStateValueOff;
            toggle.enabled = ![app.jobs containsObject:@"pack"]; [self.card addSubview:toggle];
            self.row = 60; [self finishCard];
        }
    } else if (self.page == 2) {
        [self newCard]; NSArray *providers = RTPreferences.shared.value[@"ai"][@"providers"];
        [self rowLabel:@"提供商"];
        if (providers.count) {
            [self provider]; NSMutableArray *names = NSMutableArray.array,*ids = NSMutableArray.array;
            for (NSDictionary *p in providers) { [names addObject:p[@"name"]]; [ids addObject:p[@"id"]]; }
            NSPopUpButton *picker = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(140,self.row,312,34) pullsDown:NO];
            picker.identifier = @"providerID"; picker.font = [NSFont systemFontOfSize:14]; picker.target = self; picker.action = @selector(providerChanged:);
            picker.controlSize = NSControlSizeLarge;
            for (NSUInteger i = 0; i < names.count; i++) {
                [picker addItemWithTitle:names[i]]; picker.lastItem.representedObject = ids[i];
                if ([ids[i] isEqual:self.providerID]) [picker selectItem:picker.lastItem];
            }
            [self.card addSubview:picker];
            [self iconButton:@"plus" tooltip:@"添加提供商" style:RTButtonStyleSecondary action:@selector(addProvider:) x:460];
            [self iconButton:@"trash" tooltip:@"删除当前提供商" style:RTButtonStyleDanger action:@selector(removeProvider:) x:502];
        } else {
            [self styledButton:@"添加" symbol:@"plus" tooltip:@"添加提供商" style:RTButtonStylePrimary action:@selector(addProvider:) x:140 width:96];
        }
        self.row += 42; [self finishCard];

        if (providers.count) {
            [self newCard];
            NSMutableArray *names = NSMutableArray.array; for (NSDictionary *preset in self.presets) [names addObject:preset[@"name"]];
            [self popup:@"预设" path:@"provider.preset" labels:names values:names action:@selector(preset:)];
            [self field:@"名称" path:@"provider.name" secure:NO];
            [self popup:@"接口类型" path:@"provider.kind" labels:@[@"OpenAI 兼容",@"Anthropic",@"Gemini",@"Ollama"] values:@[@"Compatible",@"Anthropic",@"Gemini",@"Ollama"] action:nil];
            [self field:@"接口地址" path:@"provider.base_url" secure:NO];
            [self field:@"API Key" path:[@"secret:ai/" stringByAppendingString:self.providerID] secure:YES];
            [self rowLabel:@"模型"];
            NSComboBox *model = [[NSComboBox alloc] initWithFrame:NSMakeRect(140,self.row,312,34)];
            model.identifier = @"testModel"; model.font = [NSFont systemFontOfSize:14]; model.delegate = self;
            model.controlSize = NSControlSizeLarge; model.editable = YES; model.selectable = YES; model.bezeled = YES;
            model.bezelStyle = NSTextFieldRoundedBezel; model.drawsBackground = YES; model.backgroundColor = NSColor.whiteColor;
            [model addItemsWithObjectValues:[self provider][@"models"]];
            model.stringValue = self.testModel ?: [[self provider][@"models"] firstObject] ?: @""; [self.card addSubview:model];
            BOOL busy = [app.jobs containsObject:@"provider"];
            [self iconButton:@"refresh-cw" tooltip:@"刷新模型列表" style:RTButtonStyleSecondary action:@selector(refreshModels:) x:460].enabled = !busy;
            [self iconButton:@"flask-conical" tooltip:@"测试接口连接" style:RTButtonStylePrimary action:@selector(testProvider:) x:502].enabled = !busy;
            self.row += 42; [self finishCard];
        }
    } else if (self.page == 3) {
        [self newCard]; [self modelRow:NO];
        [self field:@"翻译语言" path:@"ai.target" secure:NO];
        [self popup:@"思考等级" path:@"ai.reasoning" labels:@[@"不思考",@"模型默认",@"极低",@"低",@"中",@"高"] values:@[@"none",@"default",@"minimal",@"low",@"medium",@"high"] action:nil];
        [self field:@"补充要求" path:@"ai.instructions" secure:NO];
        [self field:@"超时（秒）" path:@"ai.timeout_seconds" secure:NO];
        [self divider];
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
        [self divider];
        [self toggle:@"整理口语" path:@"ai.voice.tidy"]; [self finishCard];

        [self newCard];
        NSTextField *testTitle = RTBoldLabel(@"录音测试", 14);
        testTitle.frame = NSMakeRect(20, self.row + 7, 160, 22); [self.card addSubview:testTitle];
        if (RTVoice.shared.running) {
            RTButton *stop = [self styledButton:RTVoice.shared.recording ? @"停止" : @"识别中" symbol:RTVoice.shared.recording ? @"square" : @"audio-lines" tooltip:@"结束录音并开始识别" style:RTButtonStylePrimary action:@selector(testVoice:) x:404 width:90];
            stop.enabled = RTVoice.shared.recording;
            [self iconButton:@"x" tooltip:@"取消录音" style:RTButtonStyleSecondary action:@selector(cancelVoice:) x:502];
        } else {
            [self styledButton:@"录音" symbol:@"mic" tooltip:@"开始录音测试" style:RTButtonStylePrimary action:@selector(testVoice:) x:452 width:84];
        }
        self.row += 42;
        if (RTVoice.shared.preview.length) {
            [self divider];
            NSTextField *preview = RTLabel(RTVoice.shared.preview,13);
            preview.maximumNumberOfLines = 6; preview.lineBreakMode = NSLineBreakByWordWrapping;
            preview.frame = NSMakeRect(20,self.row,516,110); [self.card addSubview:preview]; self.row += 116;
        }
        [self finishCard];
    } else if (self.page == 5) {
        [self newCard];
        NSArray *roles = @[@"mode",@"translate",@"voice"],*names = @[@"切换中英文",@"翻译",@"语音输入"];
        for (NSUInteger i = 0; i < roles.count; i++) {
            if (i > 0) [self divider];
            [self rowLabel:names[i]]; RTShortcutButton *button = [[RTShortcutButton alloc] initWithFrame:NSMakeRect(140,self.row,392,34)];
            button.title = RTShortcutLabel(RTPreferences.shared.value[@"shortcuts"][roles[i]]);
            NSString *role = roles[i]; __weak RTSettings *weakSelf = self;
            button.captured = ^(NSDictionary *binding) {
                NSMutableDictionary *keys = [RTPreferences.shared.value[@"shortcuts"] mutableCopy]; keys[role] = binding;
                NSDictionary *result = RTFeature(@{@"type":@"validateShortcuts",@"shortcuts":keys});
                if (result[@"error"]) app.messages[@5] = result[@"error"];
                else { RTPreferences.shared.value[@"shortcuts"] = keys; [RTPreferences.shared save]; app.messages[@5] = @""; }
                [weakSelf refresh];
            }; [self.card addSubview:button]; self.row += 42;
        } [self finishCard];
    } else if (self.page == 6) {
        NSDictionary *s = app.statistics;
        [self newCard]; [self rowLabel:@"版本"];
        NSPopUpButton *versions=[[NSPopUpButton alloc] initWithFrame:NSMakeRect(140,self.row,392,34) pullsDown:NO];
        versions.font = [NSFont systemFontOfSize:14]; versions.controlSize = NSControlSizeLarge;
        [versions addItemWithTitle:@"全部版本"]; versions.lastItem.representedObject=NSNull.null;
        for (NSString *version in s[@"versions"]) {
            [versions addItemWithTitle:version.length?version:@"历史未标记版本"];versions.lastItem.representedObject=version;
            if (app.statisticsVersion && [app.statisticsVersion isEqualToString:version]) [versions selectItem:versions.lastItem];
        }
        versions.identifier=@"statistics-version";versions.target=self;versions.action=@selector(statisticsVersion:);[self.card addSubview:versions];self.row+=42;[self finishCard];

        [self newCard]; [self label:@"最近 5 分钟" size:15];
        [self metric:s[@"recent"] previous:nil]; [self finishCard];

        [self newCard];
        NSTextField *avgTitle = RTBoldLabel(@"平均速度", 15);
        avgTitle.frame = NSMakeRect(20, self.row + 4, 80, 22); [self.card addSubview:avgTitle];
        NSButton *help = [NSButton buttonWithImage:[NSImage imageWithSystemSymbolName:@"questionmark.circle" accessibilityDescription:@"平均速度说明"] target:nil action:NULL];
        help.bordered = NO; help.toolTip = @"均速 = 中文字数或英文单词数 ÷ 有效输入时间；样本不足时不显示速度。";
        help.frame = NSMakeRect(92, self.row + 4, 22, 22); [self.card addSubview:help];
        NSSegmentedControl *period = [NSSegmentedControl segmentedControlWithLabels:@[@"天",@"周",@"月",@"年"] trackingMode:NSSegmentSwitchTrackingSelectOne target:self action:@selector(period:)];
        period.frame = NSMakeRect(336, self.row, 200, 30); period.selectedSegment = self.period; [self.card addSubview:period];
        self.row += 40;
        [self metric:s[@"current"] previous:s[@"previous"]];
        RTChart *chart = [[RTChart alloc] initWithFrame:NSMakeRect(20,self.row,512,160)]; chart.bars = s[@"bars"] ?: @[]; [self.card addSubview:chart]; self.row += 170;
        [self finishCard];

        for(NSUInteger i=0;i<2;i++){
            [self newCard];
            [self label:i==0?@"中文输入 · 字/次":@"英文输入 · 词/次" size:15];
            RTInputChart *inputChart=[[RTInputChart alloc] initWithFrame:NSMakeRect(20,self.row,512,216)];inputChart.unit=i==0?@"字":@"词";inputChart.rows=s[@"input"][i==0?@"chinese":@"english"]?:@[];inputChart.selectedIndex=i==0?self.chineseInputSelection:self.englishInputSelection;inputChart.identifier=i==0?@"input-chart-chinese":@"input-chart-english";
            __weak RTSettings *weakSelf=self;inputChart.selectionChanged=^(NSInteger index){if(i==0)weakSelf.chineseInputSelection=index;else weakSelf.englishInputSelection=index;};
            NSButton *summary=[NSButton buttonWithImage:[NSImage imageWithSystemSymbolName:@"questionmark.circle" accessibilityDescription:@"输入统计详情"] target:nil action:NULL];summary.bordered=NO;summary.frame=NSMakeRect(162,16,22,22);summary.toolTip=[NSString stringWithFormat:@"本%@ · %@\n%@\n取消输入 %@ 次（不计入分布）。",@[@"天",@"周",@"月",@"年"][self.period],app.statisticsVersion?(app.statisticsVersion.length?app.statisticsVersion:@"历史未标记版本"):@"全部版本",inputChart.summary,s[@"input"][@"cancelled"]?:@0];[self.card addSubview:summary];[self.card addSubview:inputChart];self.row+=224;[self finishCard];
        }

        [self newCard];
        NSTextField *voiceTitle = RTBoldLabel(@"语音输入", 14);
        voiceTitle.frame = NSMakeRect(20, self.row + 4, 120, 22); [self.card addSubview:voiceTitle];
        NSTextField *voiceStats = RTLabel([NSString stringWithFormat:@"中文 %@ 字 · 英文 %@ 词",s[@"voice"][@"chinese"] ?: @0,s[@"voice"][@"english"] ?: @0], 13);
        voiceStats.textColor = RTMuted(); voiceStats.alignment = NSTextAlignmentRight;
        voiceStats.frame = NSMakeRect(220, self.row + 4, 316, 22); [self.card addSubview:voiceStats];
        self.row += 32;
        [self finishCard];
    } else if (self.page == 7) {
        [self newCard]; [self toggle:@"云同步" path:@"cloud.enabled"];
        [self divider];
        [self popup:@"云盘" path:@"cloud.provider" labels:@[@"坚果云",@"cstcloud",@"InfiniCLOUD",@"Koofr",@"HiDrive",@"Yandex Disk",@"自定义"] values:@[@"坚果云",@"cstcloud",@"InfiniCLOUD",@"Koofr",@"HiDrive",@"Yandex Disk",@"自定义"] action:@selector(cloudPreset:)];
        [self field:@"WebDAV 地址" path:@"cloud.url" secure:NO]; [self field:@"用户名" path:@"cloud.username" secure:NO];
        [self field:@"应用密码" path:@"secret:cloud" secure:YES];
        [self field:@"设备名称" path:@"cloud.device_name" secure:NO];
        BOOL busy = [app.jobs containsObject:@"cloud"];
        [self styledButton:@"测试" symbol:@"flask-conical" tooltip:@"测试 WebDAV 连接" style:RTButtonStyleSecondary action:@selector(testCloud:) x:140 width:88].enabled = !busy;
        [self styledButton:[app.syncStatus[@"awaitingMerge"] boolValue] ? @"合并" : @"同步" symbol:@"refresh-cw" tooltip:@"立即执行云同步" style:RTButtonStylePrimary action:@selector(syncCloud:) x:236 width:96].enabled = !busy;
        self.row += 42; [self finishCard];

        for (NSDictionary *conflict in app.syncStatus[@"conflicts"]) {
            [self newCard];
            NSTextField *conflictLabel = RTBoldLabel([@"设置冲突：" stringByAppendingString:conflict[@"key"]], 14);
            conflictLabel.frame = NSMakeRect(20, self.row + 7, 300, 22); [self.card addSubview:conflictLabel];
            RTButton *local = [self styledButton:@"本机" symbol:@"laptopcomputer" tooltip:@"保留本机设置" style:RTButtonStyleSecondary action:@selector(resolveCloud:) x:368 width:80];
            local.identifier = @"local"; local.toolTip = conflict[@"key"];
            objc_setAssociatedObject(local,@selector(resolveCloud:),conflict,OBJC_ASSOCIATION_RETAIN_NONATOMIC);
            RTButton *remote = [self styledButton:@"云端" symbol:@"cloud" tooltip:@"使用云端设置" style:RTButtonStylePrimary action:@selector(resolveCloud:) x:456 width:80];
            remote.identifier = @"remote";
            objc_setAssociatedObject(remote,@selector(resolveCloud:),conflict,OBJC_ASSOCIATION_RETAIN_NONATOMIC);
            self.row += 42; [self finishCard];
        }
    } else if (self.page == 8) {
        [self renderLanPage:app];
    } else {
        [self newCard];
        NSImageView *appLogo = [[NSImageView alloc] initWithFrame:NSMakeRect(20, self.row, 40, 40)];
        appLogo.image = RTLogoImage(); appLogo.wantsLayer = YES; appLogo.layer.cornerRadius = 9; appLogo.layer.masksToBounds = YES;
        [self.card addSubview:appLogo];
        NSString *version = NSBundle.mainBundle.infoDictionary[@"CFBundleShortVersionString"] ?: @"";
        NSTextField *appName = RTBoldLabel(@"retype 输入法", 16);
        appName.frame = NSMakeRect(72, self.row + 1, 180, 22); [self.card addSubview:appName];
        NSTextField *verLabel = RTLabel(version.length ? [@"v" stringByAppendingString:version] : @"开发版", 12);
        verLabel.textColor = RTMuted(); verLabel.frame = NSMakeRect(72, self.row + 22, 180, 18); [self.card addSubview:verLabel];

        BOOL updateBusy = [app.jobs containsObject:@"updates"];
        if (app.update[@"version"]) {
            [self styledButton:@"更新" symbol:@"arrow.down.circle.fill" tooltip:@"下载并安装新版本" style:RTButtonStylePrimary action:@selector(installUpdate:) x:368 width:82].enabled = !updateBusy;
            [self iconButton:@"forward.end" tooltip:@"跳过此版本" style:RTButtonStyleSecondary action:@selector(skipUpdate:) x:458];
            [self iconButton:@"refresh-cw" tooltip:@"重新检查更新" style:RTButtonStyleSecondary action:@selector(checkUpdate:) x:500].enabled = !updateBusy;
        } else {
            [self iconButton:@"refresh-cw" tooltip:@"检查更新" style:RTButtonStyleSecondary action:@selector(checkUpdate:) x:502].enabled = !updateBusy;
        }
        self.row += 52;
        [self divider];
        [self toggle:@"自动检查更新" path:@"autoCheck"];
        [self finishCard];

        [self newCard];
        NSTextField *linksLabel = RTLabel(@"关于与支持", 14);
        linksLabel.frame = NSMakeRect(20, self.row + 7, 160, 22); [self.card addSubview:linksLabel];
        [self styledButton:@"主页" symbol:@"globe" tooltip:@"打开 GitHub 项目主页" style:RTButtonStyleSecondary action:@selector(home:) x:276 width:80];
        [self styledButton:@"许可" symbol:@"doc.text" tooltip:@"查看开源许可与第三方署名" style:RTButtonStyleSecondary action:@selector(licenses:) x:366 width:80];
        [self styledButton:@"目录" symbol:@"folder" tooltip:@"打开本地应用数据目录" style:RTButtonStyleSecondary action:@selector(dataFolder:) x:456 width:80];
        self.row += 42; [self finishCard];
    }
    self.body.frame = NSMakeRect(0,0,580,MAX(NSHeight(self.scroll.contentView.bounds),self.y));
    if (app.resourceBusy && self.page == 1 && !self.progressTimer) {
        __weak RTSettings *weakSelf = self;
        self.progressTimer = [NSTimer scheduledTimerWithTimeInterval:.25 repeats:YES block:^(NSTimer *t){(void)t;weakSelf.progress.doubleValue=rt_resources_progress(app.resources)/10.;}];
    } else if (!app.resourceBusy || self.page != 1) { [self.progressTimer invalidate];self.progressTimer=nil; }
    RTSettingsLog(@"page_render",self.page,started,YES);
}
- (void)metric:(NSDictionary *)counts previous:(NSDictionary *)previous {
    NSView *vLine = [[NSView alloc] initWithFrame:NSMakeRect(278, self.row + 4, 1, previous ? 104 : 84)];
    vLine.wantsLayer = YES; vLine.layer.backgroundColor = RTDividerColor().CGColor;
    [self.card addSubview:vLine];
    for (NSUInteger i=0;i<2;i++) {
        CGFloat x=20+i*276;NSString *speedKey=i?@"englishSpeed":@"chineseSpeed";id speed=counts[speedKey];
        NSTextField *name=RTLabel(i?@"英文":@"中文",13);name.textColor=RTMuted();name.frame=NSMakeRect(x,self.row,240,20);[self.card addSubview:name];
        NSTextField *number=RTBoldLabel(speed&&speed!=NSNull.null?[speed description]:@"—",28);number.frame=NSMakeRect(x,self.row+22,120,34);[self.card addSubview:number];
        NSTextField *unit=RTLabel(i?@"词/分钟":@"字/分钟",12);unit.textColor=RTMuted();unit.frame=NSMakeRect(x,self.row+58,240,18);[self.card addSubview:unit];
        NSNumber *old=previous[speedKey];NSString *comparison=@"上期无可比数据";
        if (speed&&speed!=NSNull.null&&old&&(id)old!=NSNull.null&&old.doubleValue>0)comparison=[NSString stringWithFormat:@"较上期 %+.0f%%",([speed doubleValue]/old.doubleValue-1)*100];
        NSString *detail = [NSString stringWithFormat:@"%@ %@ · 有效 %.1f 分钟",counts[i?@"english":@"chinese"]?:@0,i?@"词":@"字",[counts[i?@"englishMs":@"chineseMs"] doubleValue]/60000];
        if (previous) detail = [NSString stringWithFormat:@"%@ · %@", comparison, detail];
        NSTextField *count=RTLabel(detail,12);
        count.textColor=RTMuted();count.frame=NSMakeRect(x,self.row+78,248,20);[self.card addSubview:count];
    }
    self.row += 108;
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
- (void)flushCloudSecret {
    [self.window makeFirstResponder:nil];
    for (NSView *card in self.body.subviews) {
        for (NSView *sub in card.subviews) {
            if ([sub isKindOfClass:NSSecureTextField.class] && [sub.identifier isEqual:@"secret:cloud"]) {
                NSSecureTextField *f = (NSSecureTextField *)sub;
                if (f.enabled && f.stringValue.length) [self write:@"secret:cloud" value:f.stringValue];
            }
        }
    }
}
- (void)toggle:(NSSwitch *)sender {[self write:sender.identifier value:(sender.state==NSControlStateValueOn ? @YES : @NO)];if([sender.identifier isEqual:@"input.english"])[self refresh];if([sender.identifier isEqual:@"cloud.enabled"]&&sender.state==NSControlStateValueOn){[self flushCloudSecret];[(RTAppDelegate *)NSApp.delegate syncCloud:NO confirm:NO resolution:nil];}}
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
- (void)testCloud:(id)sender{(void)sender;[self flushCloudSecret];[(RTAppDelegate *)NSApp.delegate syncCloud:YES confirm:NO resolution:nil];}
- (void)syncCloud:(id)sender{(void)sender;[self flushCloudSecret];RTAppDelegate *app=(RTAppDelegate *)NSApp.delegate;[app syncCloud:NO confirm:[app.syncStatus[@"awaitingMerge"] boolValue] resolution:nil];}
- (void)resolveCloud:(NSButton *)sender{NSDictionary *c=objc_getAssociatedObject(sender,@selector(resolveCloud:));[(RTAppDelegate *)NSApp.delegate syncCloud:NO confirm:YES resolution:@{@"resolveKey":c[@"key"],@"resolveStamp":c[@"remote"][@"stamp"],@"useRemote":([sender.identifier isEqual:@"remote"] ? @YES : @NO)}];}
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
- (void)lanCommand:(id)command{
    [self.window makeFirstResponder:nil];
    RTAppDelegate *app=(RTAppDelegate *)NSApp.delegate;
    [app feature:@{@"type":@"lanCommand",@"command":command} completion:^(NSDictionary *r){
        app.messages[@8]=r[@"error"]?:@"";
        [self loadLAN];
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW,NSEC_PER_SEC),dispatch_get_main_queue(),^{[self loadLAN];});
    }];
}
- (void)lanEnable:(NSSwitch *)sender{
    BOOL on = sender.state == NSControlStateValueOn;
    if (!on) { self.lanJoining = NO; self.editingPeerID = nil; self.editingLanPort = NO; }
    [self lanCommand:@{@"Enable":@(on)}];
}
- (void)pairCode:(id)sender{(void)sender;self.lanJoining=NO;[self lanCommand:@"Pair"];}
- (void)cancelPair:(id)sender{(void)sender;[self lanCommand:@"Cancel"];}
- (void)joinDevice:(id)sender{
    (void)sender;
    self.lanJoining = !self.lanJoining;
    if (self.lanJoining) { self.lanJoinCode = @""; self.lanJoinAddress = @""; [self lanCommand:@"Cancel"]; }
    [self refresh];
}
- (void)submitJoin:(id)sender{
    (void)sender;
    [self.window makeFirstResponder:nil];
    NSString *code = [self.lanJoinCode ?: @"" stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    NSString *addr = [self.lanJoinAddress ?: @"" stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    if (code.length != 6) {
        ((RTAppDelegate *)NSApp.delegate).messages[@8] = @"请输入 6 位数字匹配码";
        [self refresh]; return;
    }
    self.lanJoining = NO; self.lanJoinCode = @""; self.lanJoinAddress = @"";
    [self lanCommand:addr.length ? @{@"JoinAddress":@{@"code":code,@"address":addr}} : @{@"Join":code}];
}
- (void)cancelJoin:(id)sender{(void)sender;[self.window makeFirstResponder:nil];self.lanJoining=NO;[self refresh];}
- (void)peerAddress:(NSButton *)sender{
    [self.window makeFirstResponder:nil];
    RTAppDelegate *app=(RTAppDelegate *)NSApp.delegate;NSString *identifier=sender.identifier;
    if ([self.editingPeerID isEqual:identifier]) { self.editingPeerID = nil; [self refresh]; return; }
    NSArray *addresses=@[];for(NSDictionary *peer in app.syncStatus[@"lan"][@"config"][@"peers"])if([peer[@"id"] isEqual:identifier])addresses=peer[@"addresses"]?:@[];
    self.editingPeerID = identifier;
    self.editingPeerAddress = [addresses componentsJoinedByString:@", "];
    [self refresh];
}
- (void)savePeerAddress:(NSButton *)sender{
    [self.window makeFirstResponder:nil];
    NSString *identifier = sender.identifier ?: self.editingPeerID;
    NSString *addr = self.editingPeerAddress ?: @"";
    self.editingPeerID = nil;
    if (identifier.length) [self lanCommand:@{@"Address":@{@"id":identifier,@"address":addr}}];
    else [self refresh];
}
- (void)cancelPeerAddress:(id)sender{(void)sender;[self.window makeFirstResponder:nil];self.editingPeerID=nil;[self refresh];}
- (void)lanPort:(id)sender{
    (void)sender;[self.window makeFirstResponder:nil];
    RTAppDelegate *app=(RTAppDelegate *)NSApp.delegate;
    NSNumber *port=app.syncStatus[@"lan"][@"config"][@"port"];
    self.lanPortValue = port.unsignedIntegerValue ? port.stringValue : @"50206";
    self.editingLanPort = YES;
    [self refresh];
}
- (void)saveLanPort:(id)sender{
    (void)sender;[self.window makeFirstResponder:nil];
    RTAppDelegate *app=(RTAppDelegate *)NSApp.delegate;
    NSInteger value = [self.lanPortValue integerValue];
    if (value > 0 && value <= 65535) {
        self.editingLanPort = NO;
        [self lanCommand:@{@"Port":@(value)}];
    } else {
        app.messages[@8] = @"端口范围为 1–65535";
        [self refresh];
    }
}
- (void)cancelLanPort:(id)sender{(void)sender;[self.window makeFirstResponder:nil];self.editingLanPort=NO;[self refresh];}
- (void)copyAddress:(NSButton *)sender{
    NSString *addr = sender.identifier;
    if (!addr.length) return;
    [NSPasteboard.generalPasteboard clearContents];
    [NSPasteboard.generalPasteboard setString:addr forType:NSPasteboardTypeString];
    ((RTAppDelegate *)NSApp.delegate).messages[@8] = @"已复制地址";
    [self refresh];
}
- (void)removePeer:(NSButton *)sender{
    if ([self.editingPeerID isEqual:sender.identifier]) self.editingPeerID = nil;
    [self lanCommand:@{@"Remove":sender.identifier}];
}
- (void)accessibility:(id)sender{(void)sender;AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)@{(__bridge id)kAXTrustedCheckOptionPrompt:@YES});}
@end
