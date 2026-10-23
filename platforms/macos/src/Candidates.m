#import "App.h"
#import <QuartzCore/QuartzCore.h>
#import <math.h>

static const CGFloat RTItemHeight = 36;
@interface RTCandidateView : NSView
@property(nonatomic, copy) NSDictionary *state;
@property(nonatomic, copy) NSArray<NSNumber *> *widths;
@property(nonatomic, weak) RTCandidatePanel *panel;
@end
@implementation RTCandidateView
- (BOOL)isFlipped { return YES; }
- (BOOL)acceptsFirstMouse:(NSEvent *)event { (void)event; return YES; }
- (void)drawRect:(NSRect)dirtyRect {
    (void)dirtyRect;
    [[NSColor colorWithCalibratedRed:.985 green:.99 blue:.995 alpha:1] setFill];
    [[NSBezierPath bezierPathWithRoundedRect:self.bounds xRadius:7 yRadius:7] fill];
    NSArray *candidates = self.state[@"candidates"];
    NSUInteger start = [self.state[@"pageStart"] unsignedIntegerValue];
    NSUInteger size = [self.state[@"pageSize"] unsignedIntegerValue];
    CGFloat x = 4;
    for (NSUInteger i = start; i < MIN(candidates.count, start + size); i++) {
        CGFloat width = MIN([self.widths[i] doubleValue], NSWidth(self.bounds) - 8);
        BOOL selected = i == [self.state[@"selected"] unsignedIntegerValue];
        NSRect cell = NSMakeRect(x, 4, width, RTItemHeight - 8);
        if (selected) {
            [[NSColor colorWithCalibratedRed:0.05 green:0.55 blue:0.59 alpha:1] setFill];
            [[NSBezierPath bezierPathWithRoundedRect:cell xRadius:5 yRadius:5] fill];
        }
        NSString *number = [NSString stringWithFormat:@"%lu", (unsigned long)(i - start + 1)];
        [number drawAtPoint:NSMakePoint(x + 7, 10) withAttributes:@{
            NSFontAttributeName: [NSFont monospacedDigitSystemFontOfSize:12 weight:NSFontWeightRegular],
            NSForegroundColorAttributeName: selected ? NSColor.whiteColor : NSColor.secondaryLabelColor }];
        NSString *text = candidates[i];
        CGFloat fontSize = 16;
        CGFloat textWidth = [text sizeWithAttributes:@{ NSFontAttributeName: [NSFont systemFontOfSize:fontSize] }].width;
        if (textWidth > width - 30) fontSize = MAX(10, fontSize * (width - 30) / textWidth);
        // One line; an unusually long single phrase scales instead of gaining an ellipsis.
        [NSGraphicsContext saveGraphicsState];
        NSRectClip(NSMakeRect(x + 22, 4, width - 26, 28));
        [text drawAtPoint:NSMakePoint(x + 22, floor((RTItemHeight - fontSize - 3) / 2)) withAttributes:@{
            NSFontAttributeName: [NSFont systemFontOfSize:fontSize weight:NSFontWeightRegular],
            NSForegroundColorAttributeName: selected ? NSColor.whiteColor : NSColor.labelColor }];
        [NSGraphicsContext restoreGraphicsState];
        x += width;
    }
}
- (void)mouseDown:(NSEvent *)event {
    NSPoint point = [self convertPoint:event.locationInWindow fromView:nil];
    NSArray *candidates = self.state[@"candidates"];
    NSUInteger start = [self.state[@"pageStart"] unsignedIntegerValue];
    CGFloat x = 4;
    for (NSUInteger i = start; i < MIN(candidates.count, start + [self.state[@"pageSize"] unsignedIntegerValue]); i++) {
        CGFloat width = MIN([self.widths[i] doubleValue], NSWidth(self.bounds) - 8);
        if (NSPointInRect(point, NSMakeRect(x, 0, width, RTItemHeight))) {
            [self.panel.owner chooseCandidate:i generation:[self.state[@"generation"] unsignedLongLongValue]];
            return;
        }
        x += width;
    }
}
@end

@implementation RTCandidatePanel
+ (instancetype)shared {
    static RTCandidatePanel *panel;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        panel = [[self alloc] initWithContentRect:NSMakeRect(0, 0, 100, RTItemHeight)
            styleMask:NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel
            backing:NSBackingStoreBuffered defer:NO];
        panel.opaque = NO;
        panel.appearance=[NSAppearance appearanceNamed:NSAppearanceNameAqua];
        panel.backgroundColor = NSColor.clearColor;
        panel.hasShadow = YES;
        panel.hidesOnDeactivate = NO;
        panel.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary;
        RTCandidateView *view = [[RTCandidateView alloc] initWithFrame:panel.contentView.bounds];
        view.panel = panel;
        view.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
        panel.contentView = view;
        panel.accessibilityLabel = @"retype 候选词";
    });
    return panel;
}
- (BOOL)canBecomeKeyWindow { return NO; }
- (BOOL)canBecomeMainWindow { return NO; }
+ (NSArray<NSNumber *> *)widths:(NSArray<NSString *> *)candidates {
    NSMutableArray *widths = [NSMutableArray arrayWithCapacity:candidates.count];
    for (NSString *text in candidates) {
        CGFloat width = ceil([text sizeWithAttributes:@{NSFontAttributeName:[NSFont systemFontOfSize:16]}].width) + 34;
        [widths addObject:@(MIN(10000, MAX(40, width)))];
    }
    return widths;
}
- (void)showState:(NSDictionary *)state anchor:(NSRect)anchor screen:(NSScreen *)screen level:(NSInteger)level owner:(id<RTCandidateOwner>)owner {
    self.owner = owner;
    NSArray *candidates = state[@"candidates"];
    if (!candidates.count) { [self orderOut:nil]; return; }
    NSArray *widths = [RTCandidatePanel widths:candidates];
    NSUInteger start = [state[@"pageStart"] unsignedIntegerValue];
    CGFloat width = 8;
    for (NSUInteger i = start; i < MIN(candidates.count, start + [state[@"pageSize"] unsignedIntegerValue]); i++) width += [widths[i] doubleValue];
    NSRect visible = screen.visibleFrame;
    width = MIN(MIN(480, NSWidth(visible) - 8), width);
    CGFloat x = MAX(NSMinX(visible) + 4, MIN(NSMinX(anchor), NSMaxX(visible) - width - 4));
    CGFloat y = NSMinY(anchor) - RTItemHeight - 4;
    if (y < NSMinY(visible)) y = NSMaxY(anchor) + 4;
    y = MAX(NSMinY(visible), MIN(y, NSMaxY(visible) - RTItemHeight));
    self.level = MAX(NSPopUpMenuWindowLevel, level + 1);
    [self setFrame:NSMakeRect(x, y, width, RTItemHeight) display:NO];
    RTCandidateView *view = (RTCandidateView *)self.contentView;
    view.state = state;
    view.widths = widths;
    view.needsDisplay = YES;
    [self orderFrontRegardless];
}
- (void)hideFor:(id<RTCandidateOwner>)owner {
    if (self.owner == owner) { [self orderOut:nil]; self.owner = nil; }
}
@end
