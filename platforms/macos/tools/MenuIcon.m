#import <Foundation/Foundation.h>
#import <CoreGraphics/CoreGraphics.h>

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc != 2) return 1;
        NSURL *url = [NSURL fileURLWithPath:[NSString stringWithUTF8String:argv[1]]];
        CGRect bounds = CGRectMake(0, 0, 18, 18);
        CGContextRef context = CGPDFContextCreateWithURL((__bridge CFURLRef)url, &bounds, NULL);
        if (!context) return 1;
        CGPDFContextBeginPage(context, NULL);
        // Reuse the r and caret from assets/logo.svg, without its colored background.
        // Center the ink bounds rather than the original app-icon canvas (the mark sits low).
        CGContextTranslateCTM(context, 9, 9);
        CGContextScaleCTM(context, 0.25, -0.25);
        CGContextTranslateCTM(context, -62.5, -66.75);
        CGContextSetGrayStrokeColor(context, 0, 1);
        CGContextSetLineCap(context, kCGLineCapRound);
        CGContextSetLineJoin(context, kCGLineJoinRound);
        CGContextSetLineWidth(context, 14);
        CGContextMoveToPoint(context, 42, 48);
        CGContextAddLineToPoint(context, 42, 88);
        CGContextMoveToPoint(context, 47, 62);
        CGContextAddCurveToPoint(context, 56, 45, 67, 42, 79, 49);
        CGContextStrokePath(context);
        CGContextSetLineWidth(context, 8);
        CGContextMoveToPoint(context, 86, 68);
        CGContextAddLineToPoint(context, 86, 87);
        CGContextStrokePath(context);
        CGPDFContextEndPage(context);
        CGPDFContextClose(context);
        CGContextRelease(context);
        return 0;
    }
}
