#import "App.h"
#include <stdio.h>
#include <stdlib.h>
extern NSPasteboard *RTTestClipboardBoard;
extern intptr_t RTTestClipboardRead(uint8_t *,size_t,uint64_t *,bool *);
extern bool RTTestClipboardWrite(const uint8_t *,size_t,bool,uint64_t,uint64_t *);
static void Check(BOOL ok,NSString *reason){if(!ok){fprintf(stderr,"Clipboard assertion: %s\n",reason.UTF8String);exit(1);}}
static BOOL Worker(BOOL (^action)(void)) {
    __block BOOL done=NO,result=NO;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED,0),^{result=action();dispatch_async(dispatch_get_main_queue(),^{done=YES;});});
    CFAbsoluteTime deadline=CFAbsoluteTimeGetCurrent()+10;
    while(!done&&CFAbsoluteTimeGetCurrent()<deadline)[NSRunLoop.mainRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:.001]];
    Check(done,@"Native clipboard callback finishes");return result;
}
@interface RTDelayedClipboard:NSObject<NSPasteboardItemDataProvider>
@property(nonatomic) NSUInteger requests;
@end
@implementation RTDelayedClipboard
- (void)pasteboard:(NSPasteboard *)board item:(NSPasteboardItem *)item provideDataForType:(NSPasteboardType)type {
    (void)board;self.requests++;[item setString:@"synthetic delayed text" forType:type];
}
@end
static void Metrics(NSString *name,NSArray<NSNumber *> *values) {
    NSArray *sorted=[values sortedArrayUsingSelector:@selector(compare:)];
    printf("%s: samples=%lu median=%.3f ms max=%.3f ms\n",name.UTF8String,(unsigned long)sorted.count,[sorted[sorted.count/2] doubleValue],[sorted.lastObject doubleValue]);
}
int main(void){@autoreleasepool{
    (void)NSApplication.sharedApplication;
    NSPasteboard *board=[NSPasteboard pasteboardWithUniqueName];RTTestClipboardBoard=board;
    RTDelayedClipboard *provider=[RTDelayedClipboard new];NSPasteboardItem *item=[NSPasteboardItem new];
    [item setDataProvider:provider forTypes:@[NSPasteboardTypeString]];[board clearContents];[board writeObjects:@[item]];
    __block uint64_t sequence=0;__block bool image=false;uint8_t *buffer=malloc(8*1024*1024);Check(buffer!=NULL,@"Test buffer allocates");
    Check(Worker(^BOOL{return RTTestClipboardRead(buffer,0,&sequence,&image)==0;}),@"Sequence-only peek returns no content");
    Check(provider.requests==0,@"Sequence-only polling must not materialize delayed clipboard data");
    NSMutableArray *copyTimes=NSMutableArray.array,*readTimes=NSMutableArray.array;
    NSString *text=[@"copy-中文-" stringByPaddingToLength:8000 withString:@"text-中文 " startingAtIndex:0];
    for(NSUInteger round=0;round<30;round++){
        NSString *value=[text stringByAppendingFormat:@"%lu",(unsigned long)round];
        CFAbsoluteTime start=CFAbsoluteTimeGetCurrent();[board clearContents];[board setString:value forType:NSPasteboardTypeString];
        Check([[board stringForType:NSPasteboardTypeString] isEqual:value],@"Local text is immediately readable");[copyTimes addObject:@((CFAbsoluteTimeGetCurrent()-start)*1000)];
        __block double elapsed=0;__block intptr_t count=0;
        Check(Worker(^BOOL{CFAbsoluteTime t=CFAbsoluteTimeGetCurrent();count=RTTestClipboardRead(buffer,8*1024*1024,&sequence,&image);elapsed=(CFAbsoluteTimeGetCurrent()-t)*1000;return count>0;}),@"Observer reads copied text");
        Check(!image&&[[[NSString alloc] initWithBytes:buffer length:(NSUInteger)count encoding:NSUTF8StringEncoding] isEqual:value],@"Observer preserves Unicode text");[readTimes addObject:@(elapsed)];
    }
    Metrics(@"text local copy+read",copyTimes);Metrics(@"text observer callback",readTimes);
    NSBitmapImageRep *bitmap=[[NSBitmapImageRep alloc] initWithBitmapDataPlanes:NULL pixelsWide:1024 pixelsHigh:768 bitsPerSample:8 samplesPerPixel:4 hasAlpha:YES isPlanar:NO colorSpaceName:NSDeviceRGBColorSpace bytesPerRow:0 bitsPerPixel:0];
    for(NSUInteger y=0;y<768;y++)for(NSUInteger x=0;x<1024;x++){unsigned char *pixel=bitmap.bitmapData+y*bitmap.bytesPerRow+x*4;pixel[0]=x%256;pixel[1]=y%256;pixel[2]=(x+y)%256;pixel[3]=255;}
    for(NSNumber *format in @[@(NSBitmapImageFileTypePNG),@(NSBitmapImageFileTypeJPEG)]){
        NSData *bytes=[bitmap representationUsingType:format.unsignedIntegerValue properties:@{NSImageCompressionFactor:@.8}];
        NSMutableArray *writes=NSMutableArray.array,*reads=NSMutableArray.array;
        for(NSUInteger round=0;round<5;round++){
            uint64_t expected=board.changeCount;__block double elapsed=0;
            Check(Worker(^BOOL{CFAbsoluteTime t=CFAbsoluteTimeGetCurrent();BOOL ok=RTTestClipboardWrite(bytes.bytes,bytes.length,true,expected,&sequence);elapsed=(CFAbsoluteTimeGetCurrent()-t)*1000;return ok;}),@"Image publication succeeds");[writes addObject:@(elapsed)];
            sequence=0;__block intptr_t count=0;
            Check(Worker(^BOOL{CFAbsoluteTime t=CFAbsoluteTimeGetCurrent();count=RTTestClipboardRead(buffer,8*1024*1024,&sequence,&image);elapsed=(CFAbsoluteTimeGetCurrent()-t)*1000;return count>0;}),@"Image observer reads");
            Check(image&&count==(intptr_t)bytes.length&&!memcmp(buffer,bytes.bytes,bytes.length),@"PNG/JPEG original bytes survive native clipboard callbacks");[reads addObject:@(elapsed)];
        }
        NSString *name=format.unsignedIntegerValue==NSBitmapImageFileTypePNG?@"PNG":@"JPEG";
        Metrics([NSString stringWithFormat:@"%@ write callback",name],writes);Metrics([NSString stringWithFormat:@"%@ read callback",name],reads);
        uint64_t stale=board.changeCount;[board clearContents];[board setString:@"new local copy" forType:NSPasteboardTypeString];
        Check(!Worker(^BOOL{return RTTestClipboardWrite(bytes.bytes,bytes.length,true,stale,&sequence);}),@"An older remote image cannot overwrite a new local copy");
        Check([[board stringForType:NSPasteboardTypeString] isEqual:@"new local copy"],@"New local text wins");
    }
    free(buffer);RTTestClipboardBoard=nil;[board releaseGlobally];
    printf("Private named pasteboard: Unicode, PNG/JPEG originals, disabled peek and local-copy precedence passed.\n");return 0;
}}
