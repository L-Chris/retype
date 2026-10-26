// Shared Lucide geometry; source, provenance and license in assets/icons/.
#import "LucideData.h"
static NSImage *RTLucideImage(NSString *name, CGFloat size, NSColor *tint) {
    static NSDictionary *icons; static dispatch_once_t once;
    dispatch_once(&once, ^{
        NSData *data=[NSData dataWithBytes:RTLucideData length:sizeof(RTLucideData)-1];
        icons=[NSJSONSerialization JSONObjectWithData:data options:0 error:NULL];
    });
    NSArray *contours=icons[name];if(!contours)return nil;
    return [NSImage imageWithSize:NSMakeSize(size,size) flipped:YES drawingHandler:^BOOL(NSRect rect) {
        (void)rect;[(tint?:NSColor.labelColor) setStroke];
        CGFloat scale=size/24.;
        NSBezierPath *path=NSBezierPath.bezierPath;
        path.lineWidth=2*scale;path.lineCapStyle=NSLineCapStyleRound;path.lineJoinStyle=NSLineJoinStyleRound;
        for(NSArray *contour in contours) {
            BOOL first=YES;
            for(NSArray *point in contour) {
                NSPoint p=NSMakePoint([point[0] doubleValue]*scale,[point[1] doubleValue]*scale);
                if(first){[path moveToPoint:p];first=NO;}else [path lineToPoint:p];
            }
        }
        [path stroke];return YES;
    }];
}
