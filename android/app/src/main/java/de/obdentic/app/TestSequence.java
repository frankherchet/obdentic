package de.obdentic.app;

/** Reviewed observation steps; recording profiles cannot supply executable steps. */
final class TestSequence {
    static final String PROFILE = "ea189-ash-test";
    enum Step {
        INVENTORY("Motorsteuergerät prüfen", "inventory", ""),
        ASH("Asche-Kandidaten einmal lesen", "ea189.ash.probe", ""),
        DPF("DPF-Vergleichswerte lesen", "ea189.dpf.probe", ""),
        RPM("Normale Messung danach prüfen", "read", "engine.rpm");

        final String label, action, argument;
        Step(String label, String action, String argument) {
            this.label=label; this.action=action; this.argument=argument;
        }
    }
    static boolean validContext(String context) { return context.equals("engine_off") || context.equals("idle"); }
    static String contextLabel(String context) {
        return context.equals("engine_off") ? "Zündung an · Motor aus" : context.equals("idle") ? "Motor im Leerlauf" : "Unbekannt";
    }
    private TestSequence() {}
}
