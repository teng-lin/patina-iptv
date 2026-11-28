#include <CoreFoundation/CoreFoundation.h>
#include <dispatch/dispatch.h>
#include <os/log.h>
#include <stdint.h>

static const char *patina_bundle_subsystem(const char *fallback) {
    static char cached[256];
    static bool has_cached = false;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        CFBundleRef main_bundle = CFBundleGetMainBundle();
        if (main_bundle) {
            CFStringRef bundle_id = CFBundleGetIdentifier(main_bundle);
            if (bundle_id && CFStringGetCString(bundle_id, cached, sizeof(cached), kCFStringEncodingUTF8)) {
                has_cached = cached[0] != '\0';
            }
        }
    });
    return has_cached ? cached : fallback;
}

void patina_apple_os_log(
    uint8_t level,
    const char *subsystem,
    const char *category,
    const char *message
) {
    os_log_type_t kind = OS_LOG_TYPE_DEFAULT;
    if (level == 1) {
        kind = OS_LOG_TYPE_ERROR;
    } else if (level >= 4) {
        kind = OS_LOG_TYPE_DEBUG;
    }
    const char *effective_sub = patina_bundle_subsystem(subsystem);
    os_log_t log = (effective_sub && effective_sub[0] && category && category[0])
        ? os_log_create(effective_sub, category)
        : OS_LOG_DEFAULT;
    os_log_with_type(log, kind, "%{public}s", message);
}
