package de.obdentic.app;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/** #231: seven signals at the 500 ms live period are read back-to-back, with no fixed gap between reads. */
final class PollScheduleAcceptance {
    static void run() {
        List<String> signals = new ArrayList<>(); for (int i = 0; i < 7; i++) signals.add("s" + i);
        Map<String, Long> due = new HashMap<>();
        require(PollSchedule.period(null) == 500, "live default period is 500 ms");
        require(PollSchedule.period(100L) == 250, "period floor is 250 ms");
        require(PollSchedule.period(1200L) == 1200, "profile interval is kept");
        long now = 0; int reads = 0;
        for (int step = 0; step < 14; step++) { // each read takes 200 ms of adapter time
            String next = PollSchedule.next(signals, due, now);
            if (next == null) break;
            require(PollSchedule.waitMs(signals, due, now) == 0, "a due signal means no waiting");
            now += 200; due.put(next, now + PollSchedule.period(null)); reads++;
        }
        require(reads >= 7, "all seven signals are read back-to-back, not one per 500 ms tick: " + reads + " reads");
        require(now == reads * 200L, "no idle gap between reads");
        due.clear(); for (String s : signals) due.put(s, 1000L);
        require(PollSchedule.next(signals, due, 400) == null, "nothing due yet");
        require(PollSchedule.waitMs(signals, due, 400) == 600, "waits only until the earliest due signal");
        require(PollSchedule.waitMs(new ArrayList<>(), due, 400) == -1, "nothing to poll");
    }
    private static void require(boolean value, String message) { if (!value) throw new AssertionError("#231 " + message); }
}
