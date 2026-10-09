import { configure } from "@testing-library/react";
import { vi } from "vitest";

// Imported by the three files that render the whole App many times (App.test.tsx, a11y.test.tsx,
// keyboard.test.tsx), and by no other: the rest keep vitest's 5 s and testing-library's 1 s.
//
// On the GitHub windows-latest runner these take 3 to 5 times as long as on Linux (a timer
// of 0 ms takes about 9 ms there, and axe sets one per rule), and a runner that stalls takes
// 10 times as long: a test of 0.5 s on Linux took 7.6 to 9.4 s (issue #72, runs 37739439198 and
// 37989694097), past the 5 s test timeout, and a findBy past its own 1 s. Neither timeout is a
// check; a test that passes passes as soon as its condition holds, so a margin only changes how
// long a real hang takes to be reported.
vi.setConfig({ testTimeout: 30_000 });
configure({ asyncUtilTimeout: 5_000 });
