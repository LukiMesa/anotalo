// Puente mínimo entre Anotalo (Rust) y whisper.cpp. Código propio: la API de whisper.cpp
// (structs grandes que cambian entre versiones) queda de este lado.
//
// Es C++ para atrapar aquí cualquier excepción de whisper.cpp o ggml (p. ej. Vulkan al crear
// el dispositivo en whisper_init_state): una excepción que llegara a Rust cerraría el programa.

#include "anotalo_whisper.h"

#include "ggml-backend.h"
#include "whisper.h"

extern "C" {

// --- carga del modelo -------------------------------------------------------------------
// whisper.cpp 1.9.5 abre las rutas UTF-8 con la API ancha de Windows (carpetas con acentos):
// lo comprueba el test loads_a_model_from_a_folder_with_accents.

void * anotalo_init(const char * model_path, int use_gpu, int gpu_device) {
    try {
        whisper_context_params cp = whisper_context_default_params();
        cp.use_gpu = use_gpu != 0;
        cp.gpu_device = gpu_device;
        cp.flash_attn = true;  // como en la prueba de velocidad (whisper-server lo trae activado)
        return whisper_init_from_file_with_params(model_path, cp);
    } catch (...) {
        return nullptr;
    }
}

// --- transcripción ----------------------------------------------------------------------

int anotalo_transcribe(void * vctx, const float * pcm, int n_samples, const char * language,
                       int n_threads, anotalo_segment_cb cb, void * user) {
    try {
        auto * ctx = static_cast<whisper_context *>(vctx);
        whisper_full_params p = whisper_full_default_params(WHISPER_SAMPLING_GREEDY);
        p.language = language;
        p.n_threads = n_threads;
        p.no_timestamps = true;
        p.no_context = true;  // sin condicionar por el texto anterior, como la app de Python
        p.print_progress = false;
        p.print_realtime = false;
        p.print_timestamps = false;
        p.print_special = false;

        const int rc = whisper_full(ctx, p, pcm, n_samples);
        if (rc != 0) {
            return rc;
        }
        const whisper_token eot = whisper_token_eot(ctx);
        const int n_segments = whisper_full_n_segments(ctx);
        for (int i = 0; i < n_segments; i++) {
            double sum = 0.0;
            int count = 0;
            const int n_tokens = whisper_full_n_tokens(ctx, i);
            for (int j = 0; j < n_tokens; j++) {
                const whisper_token_data td = whisper_full_get_token_data(ctx, i, j);
                if (td.id < eot) {  // solo texto, sin tokens especiales
                    sum += td.plog;
                    count++;
                }
            }
            const float avg_logprob = count > 0 ? static_cast<float>(sum / count) : 0.0f;
            cb(user, whisper_full_get_segment_text(ctx, i), avg_logprob,
               whisper_full_get_segment_no_speech_prob(ctx, i));
        }
        return 0;
    } catch (...) {
        return -100;
    }
}

void anotalo_free(void * ctx) {
    try {
        if (ctx != nullptr) {
            whisper_free(static_cast<whisper_context *>(ctx));
        }
    } catch (...) {
    }
}

// --- dispositivos -----------------------------------------------------------------------

int anotalo_device_count(void) {
    try {
        return static_cast<int>(ggml_backend_dev_count());
    } catch (...) {
        return 0;
    }
}

const char * anotalo_device_name(int i) {
    try {
        return ggml_backend_dev_name(ggml_backend_dev_get(static_cast<size_t>(i)));
    } catch (...) {
        return nullptr;
    }
}

const char * anotalo_device_description(int i) {
    try {
        return ggml_backend_dev_description(ggml_backend_dev_get(static_cast<size_t>(i)));
    } catch (...) {
        return nullptr;
    }
}

int anotalo_device_type(int i) {
    try {
        return static_cast<int>(ggml_backend_dev_type(ggml_backend_dev_get(static_cast<size_t>(i))));
    } catch (...) {
        return -1;
    }
}

}  // extern "C"
