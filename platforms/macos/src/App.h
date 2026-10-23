#import <Cocoa/Cocoa.h>
#import <InputMethodKit/InputMethodKit.h>
#import "retype.h"

NSDictionary *RTJSON(char *owned);
NSDictionary *RTSend(RtSession *session, NSDictionary *command);
NSString *RTDataRoot(void);
NSDictionary *RTFeature(NSDictionary *command);
NSString *RTSecret(NSString *account);
BOOL RTSaveSecret(NSString *account, NSString *value);
NSUInteger RTModifiers(NSEventModifierFlags flags);
NSString *RTShortcutLabel(NSDictionary *binding);
void RTRegisterHooks(void);

@interface RTPreferences : NSObject
@property(nonatomic, strong) NSMutableDictionary *value;
@property(nonatomic) NSUInteger revision;
+ (instancetype)shared;
- (void)save;
@end
@protocol RTEditor <NSObject>
- (NSDictionary *)captureText;
- (BOOL)replaceCapture:(NSDictionary *)capture text:(NSString *)text;
- (BOOL)insertVoice:(NSString *)text epoch:(NSUInteger)epoch;
- (NSUInteger)editorEpoch;
- (NSRect)anchorRect;
@end

@interface RTAppDelegate : NSObject <NSApplicationDelegate>
@property(nonatomic) RtRuntime *runtime;
@property(nonatomic) RtResources *resources;
@property(nonatomic) BOOL loading;
@property(nonatomic) BOOL resourceBusy;
@property(nonatomic) BOOL chinese;
@property(nonatomic, copy) NSString *resourceMessage;
- (void)loadRuntime;
- (void)showSettings:(id)sender;
- (void)runResources:(int)action source:(NSString *)source;
@property(nonatomic, weak) id<RTEditor> editor;
@property(nonatomic, strong) NSMutableDictionary *messages;
@property(nonatomic, strong) NSMutableSet *jobs;
@property(nonatomic, strong) NSDictionary *update;
@property(nonatomic, strong) NSDictionary *statistics;
@property(nonatomic, strong) NSDictionary *syncStatus;
@property(nonatomic, strong) NSTimer *maintenance;
@property(nonatomic) NSUInteger statisticsRequest;
@end
@interface RTAppDelegate (Features)
- (void)feature:(NSDictionary *)request completion:(void (^)(NSDictionary *))completion;
- (void)reloadRuntime;
- (void)translate;
- (void)voiceStartForTest:(BOOL)test;
- (void)voiceStop;
- (void)voiceCancel;
- (void)checkUpdate:(BOOL)manual;
- (void)installUpdate;
- (void)syncCloud:(BOOL)test confirm:(BOOL)confirm resolution:(NSDictionary *)resolution;
- (void)loadStatistics:(NSInteger)period;
- (void)startMaintenance;
@end

@protocol RTCandidateOwner <NSObject>
- (void)chooseCandidate:(NSUInteger)index generation:(uint64_t)generation;
@end
@interface RTCandidatePanel : NSPanel
@property(nonatomic, weak) id<RTCandidateOwner> owner;
+ (instancetype)shared;
+ (NSArray<NSNumber *> *)widths:(NSArray<NSString *> *)candidates;
- (void)showState:(NSDictionary *)state anchor:(NSRect)anchor screen:(NSScreen *)screen level:(NSInteger)level owner:(id<RTCandidateOwner>)owner;
- (void)hideFor:(id<RTCandidateOwner>)owner;
@end

@interface RTSettings : NSWindowController
+ (instancetype)shared;
+ (void)refreshIfLoaded;
- (void)refresh;
@end

@interface RetypeInputController : IMKInputController <RTCandidateOwner>
@property(nonatomic) RtSession *session;
@property(nonatomic, weak) id<IMKTextInput> activeClient;
@property(nonatomic, copy) NSDictionary *state;
@property(nonatomic, strong) NSTimer *timer;
@property(nonatomic, copy) NSString *pendingTap;
@property(nonatomic) BOOL holdingVoice;
@property(nonatomic) BOOL active;
@property(nonatomic) BOOL chinese;
@property(nonatomic) NSUInteger epoch;
@property(nonatomic, strong) NSMutableArray<NSString *> *closers;
@end
@interface RetypeInputController (Editor) <RTEditor>
- (BOOL)pairSymbol:(NSString *)key;
- (void)requestAccessibility:(id)sender;
@end

@interface RTToast : NSObject
+ (instancetype)shared;
- (void)show:(NSString *)text busy:(BOOL)busy;
- (void)hide;
@end

@interface RTVoice : NSObject
@property(atomic, readonly) BOOL running;
@property(atomic, readonly) BOOL recording;
@property(nonatomic, copy) NSString *preview;
+ (instancetype)shared;
- (void)startTest:(BOOL)test;
- (void)stop;
- (void)cancel;
@end
