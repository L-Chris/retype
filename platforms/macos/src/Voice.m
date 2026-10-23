#import "App.h"
#import <AVFoundation/AVFoundation.h>
#include <math.h>

@interface RTVoice ()
@property(atomic) BOOL running;
@property(atomic) BOOL recording;
@property(nonatomic) BOOL test;
@property(atomic) NSUInteger generation;
@property(nonatomic) NSUInteger epoch;
@property(nonatomic,weak) id<RTEditor> editor;
@property(nonatomic,copy) NSString *sessionID;
@property(nonatomic,strong) AVAudioEngine *engine;
@property(nonatomic,strong) AVAudioConverter *converter;
@property(nonatomic,strong) NSOperationQueue *queue;
@property(nonatomic,strong) NSTimer *timer;
@property(nonatomic) BOOL polling;
@end
@implementation RTVoice
+ (instancetype)shared { static RTVoice *voice;static dispatch_once_t once;dispatch_once(&once,^{voice=[self new];voice.preview=@"";voice.queue=[NSOperationQueue new];voice.queue.maxConcurrentOperationCount=1;voice.queue.name=@"retype-mac-audio";});return voice; }
- (void)startTest:(BOOL)test {
    if(self.running)return;self.generation++;NSUInteger generation=self.generation;
    self.running=YES;self.recording=YES;self.test=test;self.preview=@"";
    self.sessionID=nil;
    self.editor=((RTAppDelegate *)NSApp.delegate).editor;self.epoch=[self.editor editorEpoch];
    if(!test&&!self.editor){[self fail:@"请先将光标放到输入框中"];return;}
    [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL granted){dispatch_async(dispatch_get_main_queue(),^{
        if(!self.running||self.generation!=generation)return;
        if(!granted){[self fail:@"请在系统设置中允许 retype 使用麦克风"];return;}
        [self begin:generation];
    });}];
}
- (void)begin:(NSUInteger)generation {
    NSDictionary *config=RTPreferences.shared.value[@"ai"],*settings=config[@"voice"];
    NSString *key=RTSecret([@"ai/" stringByAppendingString:settings[@"provider"]?:@""]);
    if(!key){[self fail:@"无法读取钥匙串"];return;}
    self.sessionID=NSUUID.UUID.UUIDString;NSString *session=self.sessionID;
    RTAppDelegate *app=(RTAppDelegate *)NSApp.delegate;
    [app feature:@{@"type":@"voiceStart",@"id":session,@"config":config,@"settings":settings,@"key":key} completion:^(NSDictionary *r){
        if(!self.running||self.generation!=generation){[app feature:@{@"type":@"voiceCancel",@"id":session} completion:nil];return;}
        if(r[@"error"]){[self fail:r[@"error"]];return;}
        @try {
            self.engine=[AVAudioEngine new];AVAudioInputNode *input=self.engine.inputNode;
            AVAudioFormat *source=[input outputFormatForBus:0];
            if(source.sampleRate<=0||source.channelCount==0){[self fail:@"没有可用的麦克风"];return;}
            AVAudioFormat *target=[[AVAudioFormat alloc] initWithCommonFormat:AVAudioPCMFormatInt16 sampleRate:16000 channels:1 interleaved:YES];
            self.converter=[[AVAudioConverter alloc] initFromFormat:source toFormat:target];
            if(!self.converter){[self fail:@"无法转换麦克风音频格式"];return;}
            AVAudioConverter *converter=self.converter;__weak RTVoice *weakSelf=self;
            [input installTapOnBus:0 bufferSize:1024 format:source block:^(AVAudioPCMBuffer *buffer,AVAudioTime *when){
                (void)when;RTVoice *voice=weakSelf;if(!voice||!voice.recording)return;
                if(voice.queue.operationCount>64){dispatch_async(dispatch_get_main_queue(),^{if(voice.generation==generation)[voice fail:@"录音缓冲区已满，请重试"];});return;}
                AVAudioPCMBuffer *copy=[[AVAudioPCMBuffer alloc] initWithPCMFormat:buffer.format frameCapacity:buffer.frameLength];copy.frameLength=buffer.frameLength;
                const AudioBufferList *src=buffer.audioBufferList;AudioBufferList *dst=copy.mutableAudioBufferList;
                for(UInt32 i=0;i<src->mNumberBuffers&&i<dst->mNumberBuffers;i++){UInt32 size=MIN(src->mBuffers[i].mDataByteSize,dst->mBuffers[i].mDataByteSize);memcpy(dst->mBuffers[i].mData,src->mBuffers[i].mData,size);dst->mBuffers[i].mDataByteSize=size;}
                [voice.queue addOperationWithBlock:^{
                    if(voice.generation!=generation)return;
                    AVAudioFrameCount frames=(AVAudioFrameCount)ceil(copy.frameLength*16000/source.sampleRate)+32;
                    AVAudioPCMBuffer *out=[[AVAudioPCMBuffer alloc] initWithPCMFormat:target frameCapacity:frames];__block BOOL supplied=NO;NSError *error=nil;
                    AVAudioConverterOutputStatus status=[converter convertToBuffer:out error:&error withInputFromBlock:^AVAudioBuffer *(AVAudioPacketCount count,AVAudioConverterInputStatus *inputStatus){(void)count;if(supplied){*inputStatus=AVAudioConverterInputStatus_NoDataNow;return nil;}supplied=YES;*inputStatus=AVAudioConverterInputStatus_HaveData;return copy;}];
                    if(status==AVAudioConverterOutputStatus_Error||error){dispatch_async(dispatch_get_main_queue(),^{if(voice.generation==generation)[voice fail:@"录音转换失败"];});return;}
                    if(!out.frameLength)return;NSData *pcm=[NSData dataWithBytes:out.int16ChannelData[0] length:out.frameLength*2];
                    NSDictionary *sent=RTFeature(@{@"type":@"voicePush",@"id":session,@"pcm":[pcm base64EncodedStringWithOptions:0]});
                    if(sent[@"error"])dispatch_async(dispatch_get_main_queue(),^{if(voice.generation==generation)[voice fail:sent[@"error"]];});
                }];
            }];
            NSError *error=nil;[self.engine prepare];
            if(![self.engine startAndReturnError:&error]){[self fail:@"无法启动麦克风，请检查系统输入设备"];return;}
            self.timer=[NSTimer scheduledTimerWithTimeInterval:.12 repeats:YES block:^(NSTimer *timer){(void)timer;[weakSelf poll:generation];}];
            [RTToast.shared show:@"录音中…" busy:YES];[RTSettings refreshIfLoaded];
        }@catch(NSException *exception){(void)exception;[self fail:@"无法启动录音，请检查麦克风权限和设备"];}
    }];
}
- (void)poll:(NSUInteger)generation {
    if(self.polling||!self.running)return;self.polling=YES;NSString *session=self.sessionID;
    [self.queue addOperationWithBlock:^{NSDictionary *result=RTFeature(@{@"type":@"voicePoll",@"id":session});dispatch_async(dispatch_get_main_queue(),^{
        if(self.generation!=generation||!self.running)return;self.polling=NO;
        if(result[@"error"]){[self fail:result[@"error"]];return;}
        NSDictionary *snapshot=result[@"value"];NSString *phase=snapshot[@"phase"],*text=snapshot[@"text"]?:@"";
        BOOL changed=![text isEqual:self.preview];self.preview=text;
        if([phase isEqual:@"Error"]){[self fail:snapshot[@"error"]==NSNull.null?@"语音识别失败":snapshot[@"error"]];return;}
        if([phase isEqual:@"Recognizing"]&&self.recording){[self stopAudio];[RTSettings refreshIfLoaded];}
        if([phase isEqual:@"Done"]){
            [self stopAudio];self.running=NO;[self.timer invalidate];self.timer=nil;
            if(self.test){((RTAppDelegate *)NSApp.delegate).messages[@4]=text.length?@"识别完成":@"未识别到语音";[RTToast.shared show:text.length?@"识别完成":@"未识别到语音" busy:NO];}
            else if(text.length){if([self.editor insertVoice:text epoch:self.epoch])[RTToast.shared show:@"已输入" busy:NO];else{[NSPasteboard.generalPasteboard clearContents];[NSPasteboard.generalPasteboard setString:text forType:NSPasteboardTypeString];[RTToast.shared show:@"输入位置已变化，识别结果已复制" busy:NO];}}
            else [RTToast.shared show:@"未识别到语音" busy:NO];
            [RTSettings refreshIfLoaded];return;
        }
        NSString *message=[phase isEqual:@"Recognizing"]?@"识别中…":[NSString stringWithFormat:@"录音中… %@s",snapshot[@"seconds"]?:@0];
        if(text.length)message=[message stringByAppendingFormat:@"  %@",text];[RTToast.shared show:message busy:YES];
        if(changed&&self.test)[RTSettings refreshIfLoaded];
    });}];
}
- (void)stopAudio {
    self.recording=NO;
    @try{[self.engine.inputNode removeTapOnBus:0];[self.engine stop];}@catch(NSException *e){(void)e;}
    self.engine=nil;self.converter=nil;
}
- (void)stop {
    if(!self.running||!self.recording)return;
    if(!self.sessionID||!self.engine){[self cancel];return;}
    [self stopAudio];NSString *session=self.sessionID;NSUInteger generation=self.generation;
    [self.queue addOperationWithBlock:^{NSDictionary *r=RTFeature(@{@"type":@"voiceStop",@"id":session});if(r[@"error"])dispatch_async(dispatch_get_main_queue(),^{if(self.generation==generation)[self fail:r[@"error"]];});}];
    [RTToast.shared show:@"识别中…" busy:YES];[RTSettings refreshIfLoaded];
}
- (void)cancel {
    if(!self.running)return;self.generation++;self.running=NO;[self stopAudio];[self.timer invalidate];self.timer=nil;self.polling=NO;
    NSString *session=self.sessionID;if(session)[self.queue addOperationWithBlock:^{RTFeature(@{@"type":@"voiceCancel",@"id":session});}];
    self.sessionID=nil;[RTToast.shared hide];[RTSettings refreshIfLoaded];
}
- (void)fail:(NSString *)message {[self cancel];((RTAppDelegate *)NSApp.delegate).messages[@4]=message?:@"录音失败";[RTToast.shared show:message?:@"录音失败" busy:NO];[RTSettings refreshIfLoaded];}
@end
