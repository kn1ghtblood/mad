#include <libavformat/avformat.h>
#include <libavcodec/avcodec.h>

#define MAX_FILENAME_LENGTH 256

#ifdef _WIN32
#define EXPORT __declspec(dllexport)
#else
#define EXPORT
#endif

//int main(int argc, char *argv[]) {
EXPORT int fconcat(const char *list_file, const char *out_filename){
    AVFormatContext *output_ctx = NULL;
    AVStream *video_st = NULL, *audio_st = NULL;
    AVCodecContext *video_codec_ctx = NULL, *audio_codec_ctx = NULL;
    const AVCodec *video_codec = NULL, *audio_codec = NULL;
    AVPacket pkt;
    int ret, i;
    FILE *input_list;
    char filename[MAX_FILENAME_LENGTH];

    /* if (argc != 3) {
        fprintf(stderr, "Usage: %s <input.txt> <output.mp4>\n", argv[0]);
        return 1;
    } */

    // Initialize FFmpeg and open input file list
    //avformat_network_init();
    input_list = fopen(list_file, "r");
    if (!input_list) {
        fprintf(stderr, "Could not open input file list\n");
        return 1;
    }

    // Create output context
    avformat_alloc_output_context2(&output_ctx, NULL, NULL, out_filename);
    if (!output_ctx) {
        fprintf(stderr, "Could not create output context\n");
        return 1;
    }

    i = 0;
    int64_t video_last_pts = 0, audio_last_pts = 0;
    
    while (fgets(filename, sizeof(filename), input_list)) {
        // Remove newline character
        char *newline = strchr(filename, '\n');
        if (newline) *newline = 0;

        AVFormatContext *input_ctx = NULL;
        ret = avformat_open_input(&input_ctx, filename, NULL, NULL);
        if (ret < 0) {
            fprintf(stderr, "Could not open input file '%s'\n", filename);
            return 1;
        }

        ret = avformat_find_stream_info(input_ctx, NULL);
        if (ret < 0) {
            fprintf(stderr, "Could not find stream information\n");
            return 1;
        }

        int video_idx = -1, audio_idx = -1;

        // Find video and audio stream indexes
        for (int j = 0; j < input_ctx->nb_streams; j++) {
            if (input_ctx->streams[j]->codecpar->codec_type == AVMEDIA_TYPE_VIDEO) {
                video_idx = j;
            } else if (input_ctx->streams[j]->codecpar->codec_type == AVMEDIA_TYPE_AUDIO) {
                audio_idx = j;
            }
        }

        // Add streams to output format if this is the first file
        if (i == 0) {
            if (video_idx >= 0) {
                video_codec = avcodec_find_decoder(input_ctx->streams[video_idx]->codecpar->codec_id);
                video_st = avformat_new_stream(output_ctx, video_codec);
                avcodec_parameters_copy(video_st->codecpar, input_ctx->streams[video_idx]->codecpar);
                video_st->codecpar->codec_tag = 0;
            }
            if (audio_idx >= 0) {
                audio_codec = avcodec_find_decoder(input_ctx->streams[audio_idx]->codecpar->codec_id);
                audio_st = avformat_new_stream(output_ctx, audio_codec);
                avcodec_parameters_copy(audio_st->codecpar, input_ctx->streams[audio_idx]->codecpar);
                audio_st->codecpar->codec_tag = 0;
            }

            // Open output file
            if (!(output_ctx->oformat->flags & AVFMT_NOFILE)) {
                ret = avio_open(&output_ctx->pb, out_filename, AVIO_FLAG_WRITE);
                if (ret < 0) {
                    fprintf(stderr, "Could not open output file '%s'\n", out_filename);
                    return 1;
                }
            }

            // Write the header for the output file
            ret = avformat_write_header(output_ctx, NULL);
            if (ret < 0) {
                fprintf(stderr, "Error occurred when opening output file\n");
                return 1;
            }
        }

        // Read packets and write to the output context
        while (av_read_frame(input_ctx, &pkt) >= 0) {
            if (pkt.stream_index == video_idx) {
                pkt.stream_index = video_st->index;
                pkt.pts = pkt.dts = video_last_pts;
                video_last_pts += av_rescale_q(pkt.duration, input_ctx->streams[video_idx]->time_base, video_st->time_base);
            } else if (pkt.stream_index == audio_idx) {
                pkt.stream_index = audio_st->index;
                pkt.pts = pkt.dts = audio_last_pts;
                audio_last_pts += av_rescale_q(pkt.duration, input_ctx->streams[audio_idx]->time_base, audio_st->time_base);
            }
            pkt.pos = -1;
            ret = av_interleaved_write_frame(output_ctx, &pkt);
            if (ret < 0) {
                fprintf(stderr, "Error writing frame '%s'\n", filename);
                return 1;
            }
            av_packet_unref(&pkt);
        }

        avformat_close_input(&input_ctx);
        i++;
        //printf("Processed file: %s\n", filename);
    }

    // Write trailer and clean up
    av_write_trailer(output_ctx);
    if (!(output_ctx->oformat->flags & AVFMT_NOFILE)) {
        avio_closep(&output_ctx->pb);
    }
    avformat_free_context(output_ctx);
    fclose(input_list);

    printf("Concatenation complete. Output saved to %s\n", out_filename);
    return 0;
}
