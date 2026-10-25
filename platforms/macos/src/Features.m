#import "App.h"

@interface RTAppDelegate (FeaturePrivate)
- (void)ensurePacks;
@end

@interface RTTranslationPreview : NSObject
@property(nonatomic,strong) NSPanel *panel;
@property(nonatomic,copy) void (^apply)(void);
+ (instancetype)shared;
- (void)show:(NSString *)text apply:(void (^)(void))apply;
@end
@implementation RTTranslationPreview
+ (instancetype)shared{static RTTranslationPreview *preview;static dispatch_once_t once;dispatch_once(&once,^{preview=[self new];});return preview;}
- (void)show:(NSString *)text apply:(void (^)(void))apply {
    if(!self.panel){self.panel=[[NSPanel alloc] initWithContentRect:NSMakeRect(0,0,520,300) styleMask:NSWindowStyleMaskBorderless|NSWindowStyleMaskNonactivatingPanel backing:NSBackingStoreBuffered defer:NO];self.panel.level=NSPopUpMenuWindowLevel;self.panel.appearance=[NSAppearance appearanceNamed:NSAppearanceNameAqua];self.panel.backgroundColor=NSColor.windowBackgroundColor;self.panel.hasShadow=YES;}
    for(NSView *view in self.panel.contentView.subviews.copy)[view removeFromSuperview];self.apply=apply;
    NSTextField *title=[NSTextField labelWithString:@"翻译预览"];title.font=[NSFont systemFontOfSize:16 weight:NSFontWeightMedium];title.frame=NSMakeRect(20,256,480,24);[self.panel.contentView addSubview:title];
    NSScrollView *scroll=[[NSScrollView alloc] initWithFrame:NSMakeRect(20,62,480,180)];scroll.hasVerticalScroller=YES;scroll.drawsBackground=NO;
    NSTextView *value=[[NSTextView alloc] initWithFrame:NSMakeRect(0,0,460,180)];value.editable=NO;value.selectable=NO;value.drawsBackground=NO;value.font=[NSFont systemFontOfSize:14];value.textContainer.widthTracksTextView=YES;value.verticallyResizable=YES;value.string=text;scroll.documentView=value;[self.panel.contentView addSubview:scroll];
    NSButton *replace=[NSButton buttonWithTitle:@"替换原文" target:self action:@selector(accept:)];replace.frame=NSMakeRect(300,16,100,34);[self.panel.contentView addSubview:replace];
    NSButton *cancel=[NSButton buttonWithTitle:@"取消" target:self action:@selector(cancel:)];cancel.frame=NSMakeRect(410,16,88,34);[self.panel.contentView addSubview:cancel];
    self.panel.hidesOnDeactivate=NO;[self.panel center];[self.panel orderFrontRegardless];
}
- (void)accept:(id)sender{(void)sender;void(^apply)(void)=self.apply;self.apply=nil;[self.panel orderOut:nil];if(apply)apply();}
- (void)cancel:(id)sender{(void)sender;self.apply=nil;[self.panel orderOut:nil];}
@end

@interface RTToast ()
@property(nonatomic,strong) NSPanel *panel;
@property(nonatomic,strong) NSTextField *label;
@property(nonatomic,strong) NSProgressIndicator *spinner;
@property(nonatomic) NSUInteger generation;
@end
@implementation RTToast
+ (instancetype)shared { static RTToast *toast; static dispatch_once_t once; dispatch_once(&once,^{toast=[self new];});return toast; }
- (void)show:(NSString *)text busy:(BOOL)busy {
    if(!self.panel){self.panel=[[NSPanel alloc] initWithContentRect:NSMakeRect(0,0,220,28) styleMask:NSWindowStyleMaskBorderless|NSWindowStyleMaskNonactivatingPanel backing:NSBackingStoreBuffered defer:NO];
        self.panel.level=NSPopUpMenuWindowLevel;self.panel.appearance=[NSAppearance appearanceNamed:NSAppearanceNameAqua];self.panel.opaque=NO;self.panel.backgroundColor=[NSColor colorWithCalibratedWhite:.97 alpha:.97];self.panel.hasShadow=NO;self.panel.hidesOnDeactivate=NO;
        self.panel.collectionBehavior=NSWindowCollectionBehaviorCanJoinAllSpaces|NSWindowCollectionBehaviorFullScreenAuxiliary;
        self.label=[NSTextField labelWithString:@""];self.label.font=[NSFont systemFontOfSize:12];self.label.selectable=NO;
        self.spinner=[[NSProgressIndicator alloc] initWithFrame:NSMakeRect(8,6,16,16)];self.spinner.style=NSProgressIndicatorStyleSpinning;self.spinner.controlSize=NSControlSizeSmall;self.spinner.displayedWhenStopped=NO;
        [self.panel.contentView addSubview:self.label];[self.panel.contentView addSubview:self.spinner];
    }
    self.generation++;NSUInteger generation=self.generation;
    CGFloat width=MIN(560,MAX(100,[text sizeWithAttributes:@{NSFontAttributeName:self.label.font}].width+(busy?40:20)));
    NSRect anchor=[((RTAppDelegate *)NSApp.delegate).editor anchorRect];NSScreen *screen=NSScreen.mainScreen;
    for(NSScreen *s in NSScreen.screens)if(NSIntersectsRect(s.frame,anchor)){screen=s;break;}
    if(!screen)return;NSRect visible=screen.visibleFrame;
    CGFloat x=NSIsEmptyRect(anchor)?NSMidX(visible)-width/2:NSMinX(anchor),y=NSIsEmptyRect(anchor)?NSMaxY(visible)-90:NSMinY(anchor)-34;
    x=MAX(NSMinX(visible)+4,MIN(x,NSMaxX(visible)-width-4));y=MAX(NSMinY(visible)+4,MIN(y,NSMaxY(visible)-32));
    [self.panel setFrame:NSMakeRect(x,y,width,28) display:YES];self.label.frame=NSMakeRect(busy?30:10,5,width-(busy?38:18),19);self.label.stringValue=text;
    self.spinner.hidden=!busy;if(busy)[self.spinner startAnimation:nil];else[self.spinner stopAnimation:nil];
    if(!self.panel.visible)[self.panel orderFrontRegardless];
    if(!busy)dispatch_after(dispatch_time(DISPATCH_TIME_NOW,4*NSEC_PER_SEC),dispatch_get_main_queue(),^{if(self.generation==generation)[self hide];});
}
- (void)hide {self.generation++;[self.spinner stopAnimation:nil];[self.panel orderOut:nil];}
@end

static NSDictionary *RTCopyDocument(void){NSData *d=[NSJSONSerialization dataWithJSONObject:RTPreferences.shared.value options:0 error:NULL];return [NSJSONSerialization JSONObjectWithData:d options:NSJSONReadingMutableContainers error:NULL];}
static NSString *RTCloudAccount(NSDictionary *config){return RTFeature(@{@"type":@"cloudAccount",@"config":config})[@"value"]?:@"";}
static NSDictionary *RTInstallState(void){NSData *data=[NSData dataWithContentsOfFile:[RTDataRoot() stringByAppendingPathComponent:@"updates/install-state.json"]];id state=data?[NSJSONSerialization JSONObjectWithData:data options:0 error:NULL]:nil;return [state isKindOfClass:NSDictionary.class]?state:@{};}

@implementation RTAppDelegate (Features)
- (void)feature:(NSDictionary *)request completion:(void (^)(NSDictionary *))completion {
    [self feature:request secretAccount:nil field:nil completion:completion];
}
- (void)feature:(NSDictionary *)request secretAccount:(NSString *)account field:(NSString *)field completion:(void (^)(NSDictionary *))completion {
    NSMutableDictionary *command=[request mutableCopy];command[@"root"]=RTDataRoot();
    NSData *snapshot=[NSJSONSerialization dataWithJSONObject:command options:0 error:NULL];
    void (^execute)(NSString *)=^(NSString *secret){
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY,0),^{
            NSMutableDictionary *work=[NSJSONSerialization JSONObjectWithData:snapshot options:NSJSONReadingMutableContainers error:NULL];
            if(account)work[field]=secret;
            NSDictionary *result=RTFeature(work);
            dispatch_async(dispatch_get_main_queue(),^{if(completion)completion(result);});
        });
    };
    if(!account){execute(nil);return;}
    // Share ordering with settings writes: clicking Test immediately after
    // editing a key must use the newly saved value without blocking AppKit.
    dispatch_async(RTSecretQueue(),^{
        NSString *secret=RTSecret(account);
        if(secret)execute(secret);
        else dispatch_async(dispatch_get_main_queue(),^{if(completion)completion(@{@"error":@"无法读取钥匙串"});});
    });
}
- (void)reloadRuntime {
    if(!self.runtime)return;RtRuntime *runtime=self.runtime;NSUInteger mask=[RTPreferences.shared.value[@"packs"] unsignedIntegerValue];
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY,0),^{NSDictionary *r=RTJSON(rt_runtime_reload(runtime,RTDataRoot().UTF8String,(uint32_t)mask));
        dispatch_async(dispatch_get_main_queue(),^{if(r[@"error"])self.messages[@1]=r[@"error"];[RTSettings refreshIfLoaded];});});
}
- (void)loadStatistics:(NSInteger)period {
    NSUInteger request=++self.statisticsRequest;[self.jobs addObject:@"statistics"];
    NSString *device=RTPreferences.shared.value[@"cloud"][@"device_id"];
    [self feature:@{@"type":@"statistics",@"device":device,@"period":@(period),@"version":self.statisticsVersion?:NSNull.null} completion:^(NSDictionary *r){if(request!=self.statisticsRequest)return;[self.jobs removeObject:@"statistics"];if(r[@"error"])self.messages[@6]=r[@"error"];else self.statistics=r[@"value"];[RTSettings refreshIfLoaded];}];
}
- (void)syncCloud:(BOOL)test confirm:(BOOL)confirm resolution:(NSDictionary *)resolution {
    if([self.jobs containsObject:@"cloud"])return;
    NSDictionary *preferences=RTCopyDocument(),*config=preferences[@"cloud"];
    NSUInteger revision=RTPreferences.shared.revision;NSMutableDictionary *command=[@{@"type":test?@"syncTest":@"cloudSync",@"preferences":preferences,@"config":config,@"confirm":@(confirm)} mutableCopy];
    if(resolution)[command addEntriesFromDictionary:resolution];
    [self.jobs addObject:@"cloud"];self.messages[@7]=test?@"测试中…":@"同步中…";[RTSettings refreshIfLoaded];
    [self feature:command secretAccount:[@"sync/" stringByAppendingString:RTCloudAccount(config)] field:@"password" completion:^(NSDictionary *r){
        [self.jobs removeObject:@"cloud"];
        if(r[@"error"]){self.messages[@7]=r[@"error"];[RTSettings refreshIfLoaded];return;}
        if(test){self.messages[@7]=@"连接成功";[RTSettings refreshIfLoaded];return;}
        if(revision!=RTPreferences.shared.revision){self.messages[@7]=@"本机设置已变化，请重新同步";[RTSettings refreshIfLoaded];return;}
        NSDictionary *value=r[@"value"];NSMutableDictionary *status=[value mutableCopy];if(self.syncStatus[@"lan"])status[@"lan"]=self.syncStatus[@"lan"];self.syncStatus=status;
        self.messages[@7]=value[@"message"]?:@"同步完成";
        if(value[@"values"]){
            NSDictionary *applied=RTFeature(@{@"type":@"applyValues",@"preferences":preferences,@"values":value[@"values"]});
            if(applied[@"error"])self.messages[@7]=applied[@"error"];
            else{RTPreferences.shared.value=applied[@"value"];[RTPreferences.shared save];[self reloadRuntime];
                [self feature:@{@"type":@"mergeStatistics",@"statistics":value[@"statistics"]?:@[]} completion:^(NSDictionary *merged){if(merged[@"error"])self.messages[@7]=merged[@"error"];[RTSettings refreshIfLoaded];}];
                [self ensurePacks];
            }
        }[RTSettings refreshIfLoaded];
    }];
}
- (void)ensurePacks {
    NSArray *packs=RTFeature(@{@"type":@"packCatalog"})[@"value"];NSUInteger mask=[RTPreferences.shared.value[@"packs"] unsignedIntegerValue];
    NSMutableArray *commands=NSMutableArray.array;
    for(NSUInteger i=0;i<packs.count;i++)if(mask&(1<<i))[commands addObject:@{@"type":@"installPack",@"root":RTDataRoot(),@"id":packs[i][@"id"]}];
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY,0),^{NSString *error=nil;for(NSDictionary *command in commands){NSDictionary *r=RTFeature(command);if(r[@"error"]){error=r[@"error"];break;}}
        dispatch_async(dispatch_get_main_queue(),^{if(error)self.messages[@1]=error;[self reloadRuntime];});});
}
- (void)translate {
    if([self.jobs containsObject:@"translation"])return;
    id<RTEditor> editor=self.editor;NSDictionary *capture=[editor captureText];
    if(!capture||capture[@"error"]){[RTToast.shared show:capture[@"error"]?:@"输入框为空" busy:NO];return;}
    NSDictionary *config=RTCopyDocument()[@"ai"];
    [self.jobs addObject:@"translation"];[RTToast.shared show:@"翻译中…" busy:YES];
    [self feature:@{@"type":@"translate",@"config":config,@"text":capture[@"text"]} secretAccount:[@"ai/" stringByAppendingString:config[@"provider"]] field:@"key" completion:^(NSDictionary *r){
        [self.jobs removeObject:@"translation"];if(r[@"error"]){[RTToast.shared show:r[@"error"] busy:NO];return;}
        NSString *text=r[@"value"];if(![text isKindOfClass:NSString.class]||!text.length){[RTToast.shared show:@"没有收到译文" busy:NO];return;}
        void (^apply)(void)=^{if([editor replaceCapture:capture text:text])[RTToast.shared show:@"翻译完成" busy:NO];else{[NSPasteboard.generalPasteboard clearContents];[NSPasteboard.generalPasteboard setString:text forType:NSPasteboardTypeString];[RTToast.shared show:@"译文已复制" busy:NO];}};
        if([config[@"preview"] boolValue]){
            [RTToast.shared hide];[RTTranslationPreview.shared show:text apply:apply];
        }else apply();
    }];
}
- (void)voiceStartForTest:(BOOL)test{[RTVoice.shared startTest:test];}
- (void)voiceStop{[RTVoice.shared stop];}
- (void)voiceCancel{[RTVoice.shared cancel];}
- (void)checkUpdate:(BOOL)manual {
    if([self.jobs containsObject:@"updates"])return;[self.jobs addObject:@"updates"];if(manual){self.messages[@9]=@"检查中…";[RTSettings refreshIfLoaded];}
    [NSUserDefaults.standardUserDefaults setDouble:NSDate.date.timeIntervalSince1970 forKey:@"lastUpdateCheck"];
    [self feature:@{@"type":@"checkUpdate"} completion:^(NSDictionary *r){
        [self.jobs removeObject:@"updates"];
        if(r[@"error"]){if(manual)self.messages[@9]=r[@"error"];}
        else if(r[@"value"][@"version"]){NSString *version=r[@"value"][@"version"];if(manual||![version isEqual:RTPreferences.shared.value[@"skippedVersion"]]){self.update=r[@"value"];self.messages[@9]=[@"发现新版本 " stringByAppendingString:version];if(!manual)[RTToast.shared show:self.messages[@9] busy:NO];}}
        else{self.update=nil;if(manual)self.messages[@9]=@"已是最新可用的 macOS 版本";}[RTSettings refreshIfLoaded];
    }];
}
- (void)installUpdate {
    if(!self.update||[self.jobs containsObject:@"updates"])return;
    if(![NSBundle.mainBundle.bundlePath isEqual:[NSHomeDirectory() stringByAppendingPathComponent:@"Library/Input Methods/retype.app"]]){self.messages[@9]=@"请先安装输入法，再使用自动更新";[RTSettings refreshIfLoaded];return;}
    NSAlert *confirm=[NSAlert new];confirm.messageText=[@"安装 retype " stringByAppendingString:self.update[@"version"]];confirm.informativeText=@"安装后将重启输入法，应用和个人数据会保留。";[confirm addButtonWithTitle:@"下载并安装"];[confirm addButtonWithTitle:@"取消"];
    if([confirm runModal]!=NSAlertFirstButtonReturn)return;
    [self.jobs addObject:@"updates"];self.messages[@9]=@"正在下载安装包并校验…";RTUpdateLog(@"download_started",YES);[RTSettings refreshIfLoaded];
    [self feature:@{@"type":@"downloadUpdate",@"release":self.update} completion:^(NSDictionary *r){
        if(r[@"error"]){RTUpdateLog(@"download_failed",NO);[self.jobs removeObject:@"updates"];self.messages[@9]=r[@"error"];[RTSettings refreshIfLoaded];return;}
        RTUpdateLog(@"download_verified",YES);self.messages[@9]=@"正在解压并检查安装包…";[RTSettings refreshIfLoaded];
        NSString *archive=r[@"value"][@"path"],*version=r[@"value"][@"version"];
        NSString *helper=[NSBundle.mainBundle pathForResource:@"update" ofType:@"sh"],*bundle=NSBundle.mainBundle.bundlePath;
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY,0),^{
            NSString *directory=[RTDataRoot() stringByAppendingPathComponent:[@"updates/expand-" stringByAppendingString:NSUUID.UUID.UUIDString]];
            [NSFileManager.defaultManager createDirectoryAtPath:directory withIntermediateDirectories:YES attributes:nil error:NULL];
            NSTask *extract=[NSTask new];extract.executableURL=[NSURL fileURLWithPath:@"/usr/bin/ditto"];extract.arguments=@[@"-x",@"-k",archive,directory];
            BOOL ok=[extract launchAndReturnError:NULL];if(ok){[extract waitUntilExit];ok=extract.terminationStatus==0;}
            NSString *source=[directory stringByAppendingPathComponent:@"retype-macos/retype.app"];
            NSDictionary *info=[NSDictionary dictionaryWithContentsOfFile:[source stringByAppendingPathComponent:@"Contents/Info.plist"]];
            ok=ok&&[info[@"CFBundleIdentifier"] isEqual:@"io.github.retype.inputmethod.Retype"]&&[info[@"CFBundleShortVersionString"] isEqual:version];
            if(ok){NSTask *verify=[NSTask new];verify.executableURL=[NSURL fileURLWithPath:@"/usr/bin/codesign"];verify.arguments=@[@"--verify",@"--strict",source];ok=[verify launchAndReturnError:NULL];if(ok){[verify waitUntilExit];ok=verify.terminationStatus==0;}}
            dispatch_async(dispatch_get_main_queue(),^{
                RTUpdateLog(ok?@"package_verified":@"package_invalid",ok);
                if(!ok||!helper){[self.jobs removeObject:@"updates"];self.messages[@9]=@"更新包无效";[RTSettings refreshIfLoaded];return;}
                NSTask *installer=[NSTask new];installer.executableURL=[NSURL fileURLWithPath:@"/bin/bash"];installer.arguments=@[helper,source,bundle,[NSString stringWithFormat:@"%d",NSProcessInfo.processInfo.processIdentifier]];
                if([installer launchAndReturnError:NULL]){
                    self.updateInstaller=installer;
                    self.messages[@9]=@"正在检查安装环境…";[RTSettings refreshIfLoaded];
                    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY,0),^{
                        BOOL ready=NO;NSDate *deadline=[NSDate dateWithTimeIntervalSinceNow:15];
                        while(installer.running&&deadline.timeIntervalSinceNow>0){NSDictionary *state=RTInstallState();if([state[@"pid"] intValue]==installer.processIdentifier&&[state[@"stage"] isEqual:@"running"]&&[state[@"step"] isEqual:@"waiting_for_exit"]){ready=YES;break;}[NSThread sleepForTimeInterval:.05];}
                        if(!ready&&installer.running)[installer terminate];
                        dispatch_async(dispatch_get_main_queue(),^{
                            RTUpdateLog(ready?@"waiting_for_exit":@"installer_not_ready",ready);
                            if(ready){self.messages[@9]=@"正在保存输入记录并重启输入法…";[RTSettings refreshIfLoaded];[NSApp performSelector:@selector(terminate:) withObject:nil afterDelay:0];}
                            else{self.updateInstaller=nil;[self.jobs removeObject:@"updates"];self.messages[@9]=@"安装准备失败，请重试；详细记录见 logs/update-install.log";[RTSettings refreshIfLoaded];}
                        });
                    });
                }
                else{RTUpdateLog(@"installer_start_failed",NO);[self.jobs removeObject:@"updates"];self.messages[@9]=@"无法启动安装程序";[RTSettings refreshIfLoaded];}
            });
        });
    }];
}
- (void)startMaintenance {
    if([RTInstallState()[@"stage"] isEqual:@"failed"]){self.messages[@9]=@"上次更新安装未完成，请重试；详细记录见 logs/update-install.log";}
    __weak RTAppDelegate *weakSelf=self;
    self.maintenance=[NSTimer scheduledTimerWithTimeInterval:60 repeats:YES block:^(NSTimer *timer){(void)timer;RTAppDelegate *app=weakSelf;if(!app)return;
        NSTimeInterval now=NSDate.date.timeIntervalSince1970;
        if([RTPreferences.shared.value[@"autoCheck"] boolValue]&&now-[NSUserDefaults.standardUserDefaults doubleForKey:@"lastUpdateCheck"]>=86400)[app checkUpdate:NO];
        if([RTPreferences.shared.value[@"cloud"][@"enabled"] boolValue]&&now-[NSUserDefaults.standardUserDefaults doubleForKey:@"lastCloudSync"]>=3600){[NSUserDefaults.standardUserDefaults setDouble:now forKey:@"lastCloudSync"];[app syncCloud:NO confirm:NO resolution:nil];}
    }];
    [self ensurePacks];
    [self feature:@{@"type":@"lanResume"} completion:nil];
}
@end
