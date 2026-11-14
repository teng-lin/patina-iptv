/* Synthetic native boundary probe: no providers, rendering, or user config. */
#include <mpv/client.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int option(mpv_handle *handle, const char *name, const char *value)
{
    if (mpv_set_option_string(handle, name, value) >= 0)
        return 0;
    fprintf(stderr, "native probe could not configure %s\n", name);
    return -1;
}

int main(int argc, char **argv)
{
    if (argc != 5)
        return 2;
    const int guarded = strcmp(argv[3], "unguarded") != 0;
    const int expect_success = strcmp(argv[4], "success") == 0;
    int result = 3;
    mpv_handle *handle = mpv_create();
    if (!handle)
        return result;
    if (option(handle, "config", "no") || option(handle, "terminal", "no") ||
        option(handle, "vo", "null") || option(handle, "ao", "null") ||
        option(handle, "hwdec", "no") || option(handle, "idle", "yes") ||
        option(handle, "keep-open", "no") || option(handle, "cache", "no") || option(handle, "demuxer", "lavf"))
        goto done;
    if (guarded) {
        char avopts[4096];
        int written = snprintf(avopts, sizeof(avopts), "patina_http_prefix=%s", argv[2]);
        if (written < 0 || (size_t)written >= sizeof(avopts))
            goto done;
        if (option(handle, "demuxer-lavf-propagate-opts", "yes") ||
            option(handle, "demuxer-lavf-o", avopts) ||
            option(handle, "stream-lavf-o", avopts))
            goto done;
    }
    if (mpv_initialize(handle) < 0)
        goto done;
    if (guarded) {
        char *protocols = mpv_get_property_string(handle, "protocol-list");
        int available = protocols && strstr(protocols, "patina-relay-guard-v1");
        mpv_free(protocols);
        if (!available)
            goto done;
    }
    const char *command[] = {"loadfile", argv[1], "replace", NULL};
    if (mpv_command(handle, command) < 0)
        goto done;
    /* At most 10 seconds, even if a regression stalls rather than refusing. */
    for (int tick = 0; tick < 200; tick++) {
        mpv_event *event = mpv_wait_event(handle, 0.05);
        double position = 0;
        if (mpv_get_property(handle, "time-pos", MPV_FORMAT_DOUBLE, &position) >= 0 && position > 0.15) {
            result = expect_success ? 0 : 4;
            goto done;
        }
        if (event->event_id == MPV_EVENT_END_FILE) {
            mpv_event_end_file *ended = event->data;
            result = !expect_success && ended &&
                (ended->reason == MPV_END_FILE_REASON_ERROR ||
                 ended->reason == MPV_END_FILE_REASON_EOF) ? 0 : 5;
            goto done;
        }
    }
    result = 6;
done:
    mpv_terminate_destroy(handle);
    if (result)
        fprintf(stderr, "native probe failed (%d)\n", result);
    return result;
}
