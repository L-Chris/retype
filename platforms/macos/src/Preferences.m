#import "App.h"
#import <ImageIO/ImageIO.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#import <Security/Security.h>
#ifdef RT_UI_SMOKE
NSString *(^RTTestSecretRead)(NSString *);
BOOL (^RTTestSecretSave)(NSString *,NSString *);
#endif
dispatch_queue_t RTSecretQueue(void) {
    static dispatch_queue_t queue;static dispatch_once_t once;
    dispatch_once(&once,^{queue=dispatch_queue_create("retype.keychain",DISPATCH_QUEUE_SERIAL);});return queue;
}

NSDictionary *RTFeature(NSDictionary *command) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:command options:0 error:NULL];
    NSString *json = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
    return RTJSON(rt_feature(json.UTF8String));
}
#ifndef RT_UI_SMOKE
static NSString *const RTKeychainService = @"io.github.retype.inputmethod.Retype";
static NSString *const RTVaultAccount = @"vault-v1";
static NSMutableDictionary<NSString *, NSString *> *RTVaultCache(void) {
    static NSMutableDictionary<NSString *, NSString *> *cache;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ cache = [NSMutableDictionary new]; });
    return cache;
}
static NSMutableSet<NSString *> *RTVaultCheckedAccounts(void) {
    static NSMutableSet<NSString *> *checked;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ checked = [NSMutableSet new]; });
    return checked;
}
static NSLock *RTVaultLock(void) {
    static NSLock *lock;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ lock = [NSLock new]; });
    return lock;
}
static SecAccessRef RTVaultCreateAccess(void) {
    SecAccessRef access = NULL;
    if (SecAccessCreate(CFSTR("retype"), NULL, &access) != errSecSuccess || !access) return NULL;
    CFArrayRef acls = NULL;
    if (SecAccessCopyACLList(access, &acls) == errSecSuccess && acls) {
        CFIndex count = CFArrayGetCount(acls);
        for (CFIndex i = 0; i < count; i++) {
            SecACLRef acl = (SecACLRef)CFArrayGetValueAtIndex(acls, i);
            CFArrayRef apps = NULL;
            CFStringRef desc = NULL;
            SecKeychainPromptSelector prompt = 0;
            if (SecACLCopyContents(acl, &apps, &desc, &prompt) == errSecSuccess) {
                if (apps) {
                    CFRelease(apps);
                    SecACLSetContents(acl, NULL, desc ?: CFSTR("retype"), 0);
                }
                if (desc) CFRelease(desc);
            }
        }
        CFRelease(acls);
    }
    return access;
}
static BOOL RTVaultFlushLocked(void) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:RTVaultCache() options:0 error:NULL];
    if (!data) return NO;
    NSDictionary *query = @{
        (__bridge id)kSecClass: (__bridge id)kSecClassGenericPassword,
        (__bridge id)kSecAttrService: RTKeychainService,
        (__bridge id)kSecAttrAccount: RTVaultAccount,
    };
    NSDictionary *attributes = @{(__bridge id)kSecValueData: data};
    OSStatus status = SecItemUpdate((__bridge CFDictionaryRef)query, (__bridge CFDictionaryRef)attributes);
    if (status == errSecSuccess) return YES;
    SecItemDelete((__bridge CFDictionaryRef)query);
    NSMutableDictionary *item = [query mutableCopy];
    [item addEntriesFromDictionary:attributes];
    item[(__bridge id)kSecAttrAccessible] = (__bridge id)kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly;
    SecAccessRef access = RTVaultCreateAccess();
    if (access) {
        item[(__bridge id)kSecAttrAccess] = (__bridge id)access;
        CFRelease(access);
    }
    status = SecItemAdd((__bridge CFDictionaryRef)item, NULL);
    if (status != errSecSuccess && item[(__bridge id)kSecAttrAccess]) {
        [item removeObjectForKey:(__bridge id)kSecAttrAccess];
        status = SecItemAdd((__bridge CFDictionaryRef)item, NULL);
    }
    return status == errSecSuccess;
}
static BOOL RTVaultEnsureLoadedLocked(BOOL *authFailed) {
    static BOOL loaded = NO;
    static CFAbsoluteTime backoffUntil = 0;
    if (loaded) return YES;
    if (CFAbsoluteTimeGetCurrent() < backoffUntil) {
        if (authFailed) *authFailed = YES;
        return NO;
    }
    NSDictionary *query = @{
        (__bridge id)kSecClass: (__bridge id)kSecClassGenericPassword,
        (__bridge id)kSecAttrService: RTKeychainService,
        (__bridge id)kSecAttrAccount: RTVaultAccount,
        (__bridge id)kSecReturnData: @YES,
        (__bridge id)kSecMatchLimit: (__bridge id)kSecMatchLimitOne,
    };
    CFTypeRef result = NULL;
    OSStatus status = SecItemCopyMatching((__bridge CFDictionaryRef)query, &result);
    if (status == errSecItemNotFound) {
        loaded = YES;
        return YES;
    }
    if (status != errSecSuccess) {
        backoffUntil = CFAbsoluteTimeGetCurrent() + 60.0;
        if (authFailed) *authFailed = YES;
        return NO;
    }
    NSData *data = CFBridgingRelease(result);
    id json = data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:NULL] : nil;
    if ([json isKindOfClass:NSDictionary.class]) {
        for (id k in (NSDictionary *)json) {
            id v = ((NSDictionary *)json)[k];
            if ([k isKindOfClass:NSString.class] && [v isKindOfClass:NSString.class]) {
                RTVaultCache()[k] = v;
            }
        }
    }
    loaded = YES;
    // Re-save once if needed so existing vaults acquire the version-independent ACL.
    if (![RTVaultCache()[@"_acl_v1"] isEqual:@"1"]) {
        RTVaultCache()[@"_acl_v1"] = @"1";
        NSDictionary *delQuery = @{
            (__bridge id)kSecClass: (__bridge id)kSecClassGenericPassword,
            (__bridge id)kSecAttrService: RTKeychainService,
            (__bridge id)kSecAttrAccount: RTVaultAccount,
        };
        SecItemDelete((__bridge CFDictionaryRef)delQuery);
        RTVaultFlushLocked();
    }
    return YES;
}
#endif

NSString *RTSecret(NSString *account) {
    #ifdef RT_UI_SMOKE
    return RTTestSecretRead?RTTestSecretRead(account):@"";
    #else
    if (!account.length) return @"";
    [RTVaultLock() lock];
    BOOL authFailed = NO;
    if (!RTVaultEnsureLoadedLocked(&authFailed)) {
        [RTVaultLock() unlock];
        return authFailed ? nil : @"";
    }
    NSString *cached = RTVaultCache()[account];
    if (cached != nil) {
        [RTVaultLock() unlock];
        return cached;
    }
    if ([RTVaultCheckedAccounts() containsObject:account]) {
        [RTVaultLock() unlock];
        return @"";
    }
    [RTVaultCheckedAccounts() addObject:account];
    NSDictionary *legacyQuery = @{
        (__bridge id)kSecClass: (__bridge id)kSecClassGenericPassword,
        (__bridge id)kSecAttrService: RTKeychainService,
        (__bridge id)kSecAttrAccount: account,
        (__bridge id)kSecReturnData: @YES,
        (__bridge id)kSecMatchLimit: (__bridge id)kSecMatchLimitOne,
    };
    CFTypeRef result = NULL;
    OSStatus status = SecItemCopyMatching((__bridge CFDictionaryRef)legacyQuery, &result);
    if (status == errSecItemNotFound) {
        RTVaultCache()[account] = @"";
        [RTVaultLock() unlock];
        return @"";
    }
    if (status != errSecSuccess) {
        [RTVaultLock() unlock];
        return nil;
    }
    NSData *data = CFBridgingRelease(result);
    NSString *value = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] ?: @"";
    RTVaultCache()[account] = value;
    RTVaultCache()[@"_acl_v1"] = @"1";
    if (RTVaultFlushLocked()) {
        NSDictionary *delLegacy = @{
            (__bridge id)kSecClass: (__bridge id)kSecClassGenericPassword,
            (__bridge id)kSecAttrService: RTKeychainService,
            (__bridge id)kSecAttrAccount: account,
        };
        SecItemDelete((__bridge CFDictionaryRef)delLegacy);
    }
    [RTVaultLock() unlock];
    return value;
    #endif
}
BOOL RTSaveSecret(NSString *account, NSString *value) {
    #ifdef RT_UI_SMOKE
    return RTTestSecretSave?RTTestSecretSave(account,value):YES;
    #else
    if (!account.length) return NO;
    [RTVaultLock() lock];
    RTVaultEnsureLoadedLocked(NULL);
    [RTVaultCheckedAccounts() addObject:account];
    RTVaultCache()[account] = value ?: @"";
    RTVaultCache()[@"_acl_v1"] = @"1";
    BOOL ok = RTVaultFlushLocked();
    NSDictionary *delLegacy = @{
        (__bridge id)kSecClass: (__bridge id)kSecClassGenericPassword,
        (__bridge id)kSecAttrService: RTKeychainService,
        (__bridge id)kSecAttrAccount: account,
    };
    SecItemDelete((__bridge CFDictionaryRef)delLegacy);
    [RTVaultLock() unlock];
    return ok;
    #endif
}
static NSUserDefaults *RTDefaults(void){
    #ifdef RT_UI_SMOKE
    static NSUserDefaults *defaults;static dispatch_once_t once;dispatch_once(&once,^{defaults=[[NSUserDefaults alloc] initWithSuiteName:[@"retype-ui-validation-" stringByAppendingString:NSUUID.UUID.UUIDString]];});return defaults;
    #else
    return NSUserDefaults.standardUserDefaults;
    #endif
}
static intptr_t RTCredentialRead(const char *account,uint8_t *buffer,size_t capacity) {
    @autoreleasepool {NSString *value=RTSecret([NSString stringWithUTF8String:account]);if(!value)return -1;
        NSData *bytes=[value dataUsingEncoding:NSUTF8StringEncoding];if(bytes.length>capacity)return -1;memcpy(buffer,bytes.bytes,bytes.length);return (intptr_t)bytes.length;}
}
static bool RTCredentialSave(const char *account,const char *value) {
    @autoreleasepool {return RTSaveSecret([NSString stringWithUTF8String:account],[NSString stringWithUTF8String:value]);}
}
static intptr_t RTClipboardRead(uint8_t *buffer,size_t capacity,uint64_t *sequence,bool *image) {
    __block intptr_t count=0;
    void (^read)(void)=^{@autoreleasepool {
        NSPasteboard *pasteboard=NSPasteboard.generalPasteboard;uint64_t current=(uint64_t)pasteboard.changeCount;
        *image=false;if(*sequence==current)return;*sequence=current;
        for(NSString *type in @[@"org.nspasteboard.ConcealedType",@"org.nspasteboard.TransientType",@"org.nspasteboard.AutoGeneratedType"])if([pasteboard.types containsObject:type])return;
        NSData *data=nil;
        // Keep common encoded representations, including compressed and animated originals.
        for(NSString *type in @[UTTypeWebP.identifier,UTTypeJPEG.identifier,UTTypeGIF.identifier,NSPasteboardTypePNG,UTTypeBMP.identifier,NSPasteboardTypeTIFF]) {
            if([pasteboard.types containsObject:type]) {data=[pasteboard dataForType:type];if(data)break;}
        }
        if(data) {*image=true;} else {
            NSString *text=[pasteboard stringForType:NSPasteboardTypeString];if(!text.length)return;
            data=[text dataUsingEncoding:NSUTF8StringEncoding];if(data.length>64*1024)return;
        }
        if(data.length>capacity)return;memcpy(buffer,data.bytes,data.length);count=(intptr_t)data.length;
    }};
    if(NSThread.isMainThread)read();else dispatch_sync(dispatch_get_main_queue(),read);return count;
}
static bool RTClipboardWrite(const uint8_t *bytes,size_t length,bool image,uint64_t expected,uint64_t *sequence) {
    NSData *data=[NSData dataWithBytes:bytes length:length];
    NSString *text=image?nil:[[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
    NSString *imageType=nil;NSData *bitmapData=nil;
    if(image) {
        CGImageSourceRef source=CGImageSourceCreateWithData((__bridge CFDataRef)data,NULL);
        if(!source)return false;
        imageType=[(__bridge NSString *)CGImageSourceGetType(source) copy];
        CFRelease(source);
        if(!imageType.length)return false;
        // Apps commonly request TIFF on macOS. Keep the original as the primary type.
        if(![imageType isEqual:NSPasteboardTypeTIFF]) {
            NSBitmapImageRep *bitmap=[NSBitmapImageRep imageRepWithData:data];
            bitmapData=bitmap.TIFFRepresentation;
        }
    } else if(!text)return false;
    __block BOOL written=NO;
    void (^write)(void)=^{@autoreleasepool {
        NSPasteboard *pasteboard=NSPasteboard.generalPasteboard;if((uint64_t)pasteboard.changeCount!=expected)return;
        [pasteboard clearContents];written=image?[pasteboard setData:data forType:imageType]:[pasteboard setString:text forType:NSPasteboardTypeString];
        if(written&&bitmapData)[pasteboard setData:bitmapData forType:NSPasteboardTypeTIFF];
        *sequence=(uint64_t)pasteboard.changeCount;
    }};
    if(NSThread.isMainThread)write();else dispatch_sync(dispatch_get_main_queue(),write);return written;
}
void RTRegisterHooks(void){rt_macos_hooks(RTCredentialRead,RTCredentialSave,RTClipboardRead,RTClipboardWrite);}
static void RTNormalizeBooleans(NSMutableDictionary *root) {
    for (NSString *path in @[@"input.english",@"input.spelling",@"input.symbols",@"autoCheck",@"cloud.enabled",@"ai.preview",@"ai.voice.tidy",@"shortcuts.mode.tap",@"shortcuts.translate.tap",@"shortcuts.voice.tap"]) {
        id val = [root valueForKeyPath:path];
        if (val && val != NSNull.null) [root setValue:([val boolValue] ? @YES : @NO) forKeyPath:path];
    }
}
@implementation RTPreferences
+ (instancetype)shared {
    static RTPreferences *preferences; static dispatch_once_t once;
    dispatch_once(&once, ^{
        preferences = [self new];
        NSDictionary *defaults = RTFeature(@{@"type":@"defaults"})[@"value"];
        NSData *data = [RTDefaults() dataForKey:@"preferences-v1"];
        NSMutableDictionary *loaded = data ? [NSJSONSerialization JSONObjectWithData:data options:NSJSONReadingMutableContainers error:NULL] : nil;
        // Deep copy ensures controls can update nested form fields in place.
        NSData *fallback = [NSJSONSerialization dataWithJSONObject:defaults options:0 error:NULL];
        preferences.value = [NSJSONSerialization JSONObjectWithData:fallback options:NSJSONReadingMutableContainers error:NULL];
        if ([loaded isKindOfClass:NSDictionary.class]) [preferences.value addEntriesFromDictionary:loaded];
        else {
            NSUserDefaults *d = NSUserDefaults.standardUserDefaults;
            preferences.value[@"input"][@"scheme"] = @([d boolForKey:@"flypy"] ? 1 : 0);
            preferences.value[@"input"][@"english"] = [d boolForKey:@"englishDisabled"] ? @NO : @YES;
            preferences.value[@"input"][@"spelling"] = [d boolForKey:@"spellingDisabled"] ? @NO : @YES;
        }
        RTNormalizeBooleans(preferences.value);
        preferences.value[@"cloud"][@"device_name"] = NSHost.currentHost.localizedName ?: @"Mac";
        [preferences save];
    });
    return preferences;
}
- (void)save {
    self.revision++;
    RTNormalizeBooleans(self.value);
    NSData *data = [NSJSONSerialization dataWithJSONObject:self.value options:0 error:NULL];
    if (data) [RTDefaults() setObject:data forKey:@"preferences-v1"];
    [[NSNotificationCenter defaultCenter] postNotificationName:@"RTPreferencesChanged" object:nil];
}
@end
NSUInteger RTModifiers(NSEventModifierFlags flags) {
    return ((flags & NSEventModifierFlagControl) ? 1 : 0) | ((flags & NSEventModifierFlagOption) ? 2 : 0) |
        ((flags & NSEventModifierFlagShift) ? 4 : 0) | ((flags & NSEventModifierFlagCommand) ? 8 : 0);
}
NSString *RTShortcutLabel(NSDictionary *binding) {
    NSInteger key = [binding[@"key"] integerValue], mods = [binding[@"mods"] integerValue];
    NSDictionary *names = @{@56:@"Shift",@60:@"右 Shift",@59:@"Control",@62:@"右 Control",@58:@"Option",@61:@"右 Option",@49:@"Space",@29:@"0",@18:@"1",@19:@"2",@20:@"3",@21:@"4",@23:@"5",@22:@"6",@26:@"7",@28:@"8",@25:@"9",@0:@"A",@1:@"S",@2:@"D",@3:@"F",@4:@"H",@5:@"G",@6:@"Z",@7:@"X",@8:@"C",@9:@"V",@11:@"B",@12:@"Q",@13:@"W",@14:@"E",@15:@"R",@16:@"Y",@17:@"T",@31:@"O",@32:@"U",@34:@"I",@35:@"P",@37:@"L",@38:@"J",@40:@"K",@45:@"N",@46:@"M"};
    NSMutableArray *parts = NSMutableArray.array;
    if (mods & 1) [parts addObject:@"⌃"]; if (mods & 2) [parts addObject:@"⌥"];
    if (mods & 4) [parts addObject:@"⇧"]; if (mods & 8) [parts addObject:@"⌘"];
    NSArray *functionKeys=@[@122,@120,@99,@118,@96,@97,@98,@100,@101,@109,@103,@111,@105,@107,@113,@106,@64,@79,@80,@90];
    NSUInteger f=[functionKeys indexOfObject:@(key)];
    NSDictionary *extra=@{@36:@"Return",@48:@"Tab",@53:@"Esc",@51:@"⌫",@117:@"⌦",@123:@"←",@124:@"→",@125:@"↓",@126:@"↑"};
    [parts addObject:names[@(key)] ?: extra[@(key)] ?: (f!=NSNotFound?[NSString stringWithFormat:@"F%lu",(unsigned long)f+1]:[NSString stringWithFormat:@"Key %ld",(long)key])];
    return [parts componentsJoinedByString:@" "];
}
