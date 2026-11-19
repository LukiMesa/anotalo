//! Sin auriculares el micrófono oye a los demás por los altavoces: lo que dicen vuelve a salir como
//! tuyo. `echo_in` decide qué queda de una línea tuya.
use anotalo_core::echo_lines::{Echo, Said, echo_in};

fn said(start_s: f64, end_s: f64, text: &str) -> Said<'_> {
    Said {
        start_s,
        end_s,
        text,
    }
}

fn part(e: Echo) -> (f64, String) {
    match e {
        Echo::Part { start_s, text } => (start_s, text),
        other => panic!("se esperaba una parte, salió {other:?}"),
    }
}

#[test]
fn test_whole_line_repeated_is_echo() {
    let mine = said(
        10.2,
        14.0,
        "Necesitamos una tienda virtual para vender los productos",
    );
    let others = [said(
        10.0,
        14.1,
        "Necesitamos una tienda virtual para vender los productos.",
    )];
    assert_eq!(echo_in(&mine, &others), Echo::All);
}

#[test]
fn test_echo_survives_small_transcription_differences() {
    let mine = said(
        50.0,
        58.0,
        "y que lo agregue pues como el precio total del producto para que no se pierda",
    );
    let others = [said(
        49.0,
        58.5,
        "Y que lo agregue, pues, como al precio total de los productos para que pues no se pierda ese dinero.",
    )];
    assert_eq!(echo_in(&mine, &others), Echo::All);
}

#[test]
fn test_echo_split_across_several_lines_of_the_other() {
    let mine = said(
        20.0,
        30.0,
        "la idea es que el sitio sea el aterrizaje de todo lo que se haga en redes",
    );
    let others = [
        said(18.0, 24.0, "Bueno, la idea es que el sitio sea"),
        said(24.5, 31.0, "el aterrizaje de todo lo que se haga en redes."),
    ];
    assert_eq!(echo_in(&mine, &others), Echo::All);
}

#[test]
fn test_reply_after_the_echo_is_kept() {
    let mine = said(
        100.0,
        112.0,
        "para que te queden solo veinte a la venta, ¿cierto? Sí, así en vez de manejar una hoja de cálculo lo llevas desde la página.",
    );
    let others = [said(
        99.0,
        104.0,
        "esos treinta para que te queden solo veinte a la venta, ¿cierto?",
    )];
    let (start, text) = part(echo_in(&mine, &others));
    assert_eq!(
        text,
        "Sí, así en vez de manejar una hoja de cálculo lo llevas desde la página."
    );
    assert!(start > 100.0 && start < 112.0, "{start}");
}

#[test]
fn test_reply_before_the_echo_is_kept() {
    let mine = said(
        200.0,
        209.0,
        "Y pues les miro varias partes. Yo te puedo pasar toda la información de la tienda.",
    );
    let others = [said(
        202.0,
        209.5,
        "Yo te puedo pasar toda la información de la tienda.",
    )];
    assert_eq!(
        part(echo_in(&mine, &others)),
        (200.0, "Y pues les miro varias partes.".to_string())
    );
}

#[test]
fn test_short_reply_after_echo_with_similar_words() {
    // "producto" frente a "productos": la misma palabra mal oída
    let mine = said(
        300.0,
        305.0,
        "líneas de producto sería por categoría sí, sí, claro",
    );
    let others = [said(
        298.0,
        304.0,
        "el tema de esas líneas de productos sería por categoría",
    )];
    assert_eq!(part(echo_in(&mine, &others)).1, "sí, sí, claro");
}

#[test]
fn test_echo_is_matched_to_the_line_that_says_it() {
    let mine = said(1719.0, 1722.0, "Sí, yo creo que sí es importante la gente");
    let others = [
        said(
            1700.0,
            1716.0,
            "Sí, porque si necesitamos algo yo me encargo de una",
        ),
        said(1718.0, 1721.0, "Sí, yo creo que sí es importante la gente."),
    ];
    assert_eq!(echo_in(&mine, &others), Echo::All);
}

#[test]
fn test_reply_sharing_words_with_the_next_line_is_kept() {
    let mine = said(
        1311.0,
        1322.0,
        "No sé si conoces la plataforma de prueba, ¿no? Creo que sí, la he visto.",
    );
    let others = [
        said(1311.0, 1318.0, "No sé, si conoces la plataforma de prueba."),
        said(1322.0, 1327.0, "Bueno, te recomiendo que la mires."),
    ];
    assert_eq!(
        part(echo_in(&mine, &others)).1,
        "¿no? Creo que sí, la he visto."
    );
}

#[test]
fn test_common_word_pairs_from_different_places_are_not_echo() {
    let mine = said(
        952.0,
        958.0,
        "y que no hayan problemas con que se pidan cosas que no están",
    );
    let others = [said(
        950.0,
        960.0,
        "que no se te olvide revisar que se vea bien en el celular",
    )];
    assert_eq!(echo_in(&mine, &others), Echo::No);
}

#[test]
fn test_own_words_are_not_echo() {
    let mine = said(
        40.0,
        45.0,
        "Me parece bien, yo me encargo del inventario desde el panel",
    );
    let others = [said(38.0, 40.5, "Entonces, ¿qué te parece?")];
    assert_eq!(echo_in(&mine, &others), Echo::No);
}

#[test]
fn test_same_words_long_before_are_not_echo() {
    let mine = said(
        500.0,
        504.0,
        "Necesitamos una tienda virtual para vender los productos",
    );
    let others = [said(
        10.0,
        14.0,
        "Necesitamos una tienda virtual para vender los productos.",
    )];
    assert_eq!(echo_in(&mine, &others), Echo::No);
}

#[test]
fn test_short_reply_sharing_a_word_is_kept() {
    let mine = said(60.0, 61.5, "Listo, perfecto.");
    let others = [said(59.0, 60.0, "Listo.")];
    assert_eq!(echo_in(&mine, &others), Echo::No);
}

#[test]
fn test_short_exact_repeat_at_the_same_moment_is_echo() {
    let mine = said(70.4, 71.5, "Bueno chicos, entonces");
    let others = [said(70.0, 74.0, "Bueno chicos, entonces a lo que vinimos.")];
    assert_eq!(echo_in(&mine, &others), Echo::All);
}

#[test]
fn test_scattered_common_words_are_not_echo() {
    let mine = said(
        80.0,
        90.0,
        "Pues yo tenía pensado en la página un apartado para manejar el inventario desde un acceso de administrador",
    );
    let others = [said(
        79.0,
        88.0,
        "que la página de la tienda tenga todo el portafolio de los productos y que se vea en el celular",
    )];
    assert_eq!(echo_in(&mine, &others), Echo::No);
}

#[test]
fn test_one_word_is_never_echo() {
    let mine = said(90.0, 90.5, "Prueba");
    let others = [said(89.5, 91.0, "¿Cómo, Prueba?")];
    assert_eq!(echo_in(&mine, &others), Echo::No);
}

#[test]
fn test_nothing_said_by_others_is_not_echo() {
    let mine = said(1.0, 3.0, "Hola, ¿me escuchan?");
    assert_eq!(echo_in(&mine, &[]), Echo::No);
}
