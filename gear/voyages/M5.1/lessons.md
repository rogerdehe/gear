# M5.1 lessons

1. A contract that says "as path X does" names what turns path X on (T-438, F-227: "as the result path does for a lane" left out that the lane's check runs only when the CLI builds a gate, so the lane asked, and the direction check proposed a reading that would never run). Read the reused path to its trigger and write the trigger, and every file it lives in, into the contract.
2. An answer that widens a contract goes into the contract too (T-438: the answer to M-6367 allowed a line in gear-cli/src/main.rs, the contract still said "these files only", and the reviewer failed the task for that line). When an answer adds a file or changes the design, bump the contract in the same step, so the reviewer judges what the lane was told.
