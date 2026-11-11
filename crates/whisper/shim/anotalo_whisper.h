/* Puente mínimo entre Anotalo (Rust) y whisper.cpp. Código propio: la API de whisper.cpp
 * (structs grandes que cambian entre versiones) queda de este lado, en C. */
#ifndef ANOTALO_WHISPER_H
#define ANOTALO_WHISPER_H

#ifdef __cplusplus
extern "C" {
#endif

/* Carga el modelo (ruta en UTF-8). NULL si falla. */
void * anotalo_init(const char * model_path, int use_gpu, int gpu_device);

typedef void (*anotalo_segment_cb)(void * user, const char * text, float avg_logprob, float no_speech_prob);

/* Transcribe audio mono 16 kHz. Llama a cb por cada segmento. 0 = bien. */
int anotalo_transcribe(void * ctx, const float * pcm, int n_samples, const char * language,
                       int n_threads, anotalo_segment_cb cb, void * user);

void anotalo_free(void * ctx);

/* Dispositivos de cálculo (ggml): 0 CPU, 1 GPU, 2 IGPU, 3 ACCEL, 4 META */
int anotalo_device_count(void);
const char * anotalo_device_name(int i);
const char * anotalo_device_description(int i);
int anotalo_device_type(int i);

#ifdef __cplusplus
}
#endif

#endif
