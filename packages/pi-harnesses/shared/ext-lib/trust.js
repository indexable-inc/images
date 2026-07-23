// Earned-trust check-in interval for the prosecutor harness.
//
// Pi has no native turn/step budget, so the check-in cadence lives here. Start
// tight (verify every action). Each upheld claim doubles the interval up to a
// ceiling; a single broken claim snaps supervision back to every action. A
// competent run accelerates toward autonomy; a confused run gets babysat
// harder, with no magic constant to tune by hand.
//
// Pure and synchronous so it can be unit-tested without Pi.
export function createTrust({ min = 1, max = 16 } = {}) {
  let interval = min;
  let streak = 0;
  return {
    get interval() {
      return interval;
    },
    get streak() {
      return streak;
    },
    record(upheld) {
      if (upheld) {
        streak += 1;
        interval = Math.min(interval * 2, max);
      } else {
        streak = 0;
        interval = min;
      }
      return { interval, streak, upheld };
    },
  };
}
