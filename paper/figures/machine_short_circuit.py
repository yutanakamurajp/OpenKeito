"""Three-phase short circuit at the terminals of a synchronous machine
against Park's equations integrated separately.

The 555 MVA, 24 kV, 60 Hz generator of Kundur's examples (Xd = 1.81,
Xq = 1.76, Xd' = 0.30, Xq' = 0.65, Xd'' = 0.23, Xq'' = 0.25, Xl = 0.15,
Ra = 0.003, Td0' = 8 s, Td0'' = 0.03 s, Tq0' = 1 s, Tq0'' = 0.07 s,
H = 3.5 s; unsaturated) runs at no load and rated voltage; at 10 ms its
terminals are shorted through 2 mohm per phase.

The reference shares nothing with the solver: the flux linkages of the d
axis (stator, field, one damper) and of the q axis (stator, two dampers),
the speed and the rotor angle, in per unit (Park 1929; Kundur 1994, ch. 3
to 5), with the terminal voltage zero behind the fault resistance,

    d psi_d/dt  = w0 (w psi_q + r i_d),        d psi_q/dt = w0 (-w psi_d + r i_q),
    d psi_fd/dt = w0 (e_fd - R_fd i_fd),       d psi_k/dt = -w0 R_k i_k  (dampers),
    2H dw/dt    = -(psi_d i_q - psi_q i_d),    d delta/dt = w0 (w - 1),

integrated by the classical Runge-Kutta method with a step of 5 us. The
winding inductances and resistances come from the standard parameters by
the classical expressions. The phase current is
i_a = -(i_d cos(theta) - i_q sin(theta)), theta = w0 t + delta - pi/2.
"""

from __future__ import annotations

import math

import numpy as np

import lib
import style

TITLE = "Terminal short circuit of a synchronous machine against Park's equations integrated separately"
W0 = 2 * math.pi * 60.0
S_BASE, V_LL = 555e6, 24e3
Z_BASE = V_LL * V_LL / S_BASE
I_BASE = math.sqrt(2.0 / 3.0) * S_BASE / V_LL
M = {"xd": 1.81, "xq": 1.76, "xd1": 0.30, "xq1": 0.65, "xd2": 0.23, "xq2": 0.25, "xl": 0.15, "ra": 0.003,
     "td01": 8.0, "td02": 0.03, "tq01": 1.0, "tq02": 0.07, "h": 3.5}
T_FAULT, R_FAULT = 0.01, 2e-3
SIGNALS = ["id_pu", "iq_pu", "field_current_pu", "speed_pu", "angle_rad"]


def circuit(method: str, h: float, end: float) -> dict:
    components = [
        {"type": "synchronous_machine", "name": "G1", "bus": "b1", "rated_power_mva": S_BASE / 1e6,
         "rated_voltage_kv": V_LL / 1e3, "frequency_hz": 60.0, "xd_pu": M["xd"], "xq_pu": M["xq"],
         "xd_transient_pu": M["xd1"], "xq_transient_pu": M["xq1"], "xd_subtransient_pu": M["xd2"],
         "xq_subtransient_pu": M["xq2"], "xl_pu": M["xl"], "ra_pu": M["ra"], "td0_transient_s": M["td01"],
         "td0_subtransient_s": M["td02"], "tq0_transient_s": M["tq01"], "tq0_subtransient_s": M["tq02"],
         "inertia_h_s": M["h"], "damping_pu": 0.0},
        {"type": "fault", "name": "F1", "bus": "b1", "phases": ["a", "b", "c"], "ground": True,
         "fault_resistance_ohm": R_FAULT, "ground_resistance_ohm": 1e-3, "start_time_s": T_FAULT}]
    components += [{"type": "resistor", "name": f"R{p}", "nodes": [f"b1.{p}", "gnd"], "resistance_ohm": 1e6}
                   for p in "abc"]
    controls = [{"type": "measurement", "name": n,
                 "quantity": {"type": "component_signal", "component": "G1", "signal": n}} for n in SIGNALS]
    outputs = [{"type": "signal", "signal": n} for n in SIGNALS] + [{"type": "branch_current", "component": "G1"}]
    return lib.model({"time_step_s": h, "end_time_s": end, "method": method, "initialization": "steady_state"},
                     components, outputs, controls)


class Park:
    """Park's equations of the shorted machine. State: psi_d, psi_fd,
    psi_1d, psi_q, psi_1q, psi_2q, speed, angle."""

    def __init__(self) -> None:
        def two(x, x1, x2, t1, t2):
            lm = x - M["xl"]
            l1 = lm * (x1 - M["xl"]) / (x - x1)
            l2 = (x1 - M["xl"]) * (x2 - M["xl"]) / (x1 - x2)
            return [l1, l2], [(lm + l1) / (W0 * t1), (l2 + x1 - M["xl"]) / (W0 * t2)]

        def matrix(lm, leak):
            l = np.full((3, 3), lm)
            l[0, 0] += M["xl"]
            l[1, 1] += leak[0]
            l[2, 2] += leak[1]
            return l

        ld, self.r_d = two(M["xd"], M["xd1"], M["xd2"], M["td01"], M["td02"])
        lq, self.r_q = two(M["xq"], M["xq1"], M["xq2"], M["tq01"], M["tq02"])
        self.lad = M["xd"] - M["xl"]
        self.l_d = matrix(self.lad, ld)
        self.inv_d = np.linalg.inv(self.l_d)
        self.inv_q = np.linalg.inv(matrix(M["xq"] - M["xl"], lq))
        self.r = M["ra"] + R_FAULT / Z_BASE
        # No load at rated voltage: i_fd = 1 / L_ad.
        self.e_fd = self.r_d[0] / self.lad

    def no_load(self) -> np.ndarray:
        return np.array([1.0, self.l_d[1, 1] / self.lad, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0])

    def currents(self, y: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        """[i_d, i_fd, i_1d], [i_q, i_1q, i_2q]; the stator currents are
        positive out of the machine."""
        d, q = self.inv_d @ y[:3], self.inv_q @ y[3:6]
        d[0], q[0] = -d[0], -q[0]
        return d, q

    def derivative(self, y: np.ndarray) -> np.ndarray:
        d, q = self.currents(y)
        psi_d, psi_q, w = y[0], y[3], y[6]
        torque = psi_d * q[0] - psi_q * d[0]
        return np.array([
            W0 * (w * psi_q + self.r * d[0]),
            W0 * (self.e_fd - self.r_d[0] * d[1]),
            W0 * (-self.r_d[1] * d[2]),
            W0 * (-w * psi_d + self.r * q[0]),
            W0 * (-self.r_q[0] * q[1]),
            W0 * (-self.r_q[1] * q[2]),
            -torque / (2 * M["h"]),
            W0 * (w - 1.0)])

    def step(self, y: np.ndarray, dt: float) -> np.ndarray:
        k1 = self.derivative(y)
        k2 = self.derivative(y + dt / 2 * k1)
        k3 = self.derivative(y + dt / 2 * k2)
        k4 = self.derivative(y + dt * k3)
        return y + dt / 6 * (k1 + 2 * k2 + 2 * k3 + k4)


def reference(t: np.ndarray, per_row: int, dt: float) -> dict[str, np.ndarray]:
    """The reference at the rows t (from the instant of the fault on)."""
    park = Park()
    y = park.no_load()
    out = {k: [] for k in ("ia", "id", "iq", "ifd", "speed", "angle")}
    for row, time in enumerate(t):
        if row > 0:
            for _ in range(per_row):
                y = park.step(y, dt)
        d, q = park.currents(y)
        theta = W0 * time + y[7] - math.pi / 2
        for k, v in (("ia", -(d[0] * math.cos(theta) - q[0] * math.sin(theta))), ("id", d[0]), ("iq", q[0]),
                     ("ifd", park.lad * d[1]), ("speed", y[6]), ("angle", y[7])):
            out[k].append(v)
    return {k: np.array(v) for k, v in out.items()}


def make(ctx: lib.Context) -> dict:
    end = 0.06 if ctx.quick else 0.21
    dt = 5e-6
    steps = [100e-6, 50e-6]
    methods = ["2s-dirk"] if ctx.quick else ["2s-dirk", "trapezoidal"]
    # One reference at the rows of the smallest step; the larger step reads
    # every second row.
    first = int(round(T_FAULT / steps[-1]))
    n_rows = int(round(end / steps[-1])) + 1
    t_ref = np.arange(first, n_rows) * steps[-1]
    ref = reference(t_ref, int(round(steps[-1] / dt)), dt)

    rows, errors, waves = [], {}, {}
    for method in methods:
        for h in steps:
            r = lib.run(circuit(method, h, end))
            stride = int(round(h / steps[-1]))
            k0 = int(round(T_FAULT / h))
            pick = {k: v[::stride] for k, v in ref.items()}
            n = min(len(r.time) - k0, len(pick["ia"]))
            e = {
                "ia": float(np.max(np.abs(r["I(G1.a)"][k0:k0 + n] / I_BASE - pick["ia"][:n]))),
                "idq": float(max(np.max(np.abs(r["S(id_pu)"][k0:k0 + n] - pick["id"][:n])),
                                 np.max(np.abs(r["S(iq_pu)"][k0:k0 + n] - pick["iq"][:n])))),
                "ifd": float(np.max(np.abs(r["S(field_current_pu)"][k0:k0 + n] - pick["ifd"][:n]))),
                "speed": float(np.max(np.abs(r["S(speed_pu)"][k0:k0 + n] - pick["speed"][:n]))),
            }
            errors[(method, h)] = e
            solver = r.summary["solver"]
            rows.append((method, h, e["ia"], e["idq"], e["ifd"], e["speed"],
                         solver["operating_point_iterations"] / (2 * solver["steps"]),
                         solver["max_operating_point_iterations_per_stage"]))
            waves[(method, h)] = (r, k0, n, pick)
    lib.write_csv(ctx, "machine_short_circuit.csv",
                  ["method", "time_step_s", "max_error_phase_current_pu", "max_error_dq_currents_pu",
                   "max_error_field_current_pu", "max_error_speed_pu", "stage_iterations_per_stage",
                   "max_stage_iterations"], rows)
    r, k0, n, pick = waves[("2s-dirk", 100e-6)]
    t = r.time[k0:k0 + n]
    ia, ifd = r["I(G1.a)"][k0:k0 + n] / I_BASE, r["S(field_current_pu)"][k0:k0 + n]
    lib.write_csv(ctx, "machine_short_circuit_waveform.csv",
                  ["time_s", "i_a_park_pu", "i_a_openkeito_pu", "i_fd_park_pu", "i_fd_openkeito_pu"],
                  zip(t, pick["ia"][:n], ia, pick["ifd"][:n], ifd))

    fig, (ax1, ax2) = style.figure(2, 1, width="double", height_in=3.1)
    ms = t * 1e3
    every = 6
    ax1.plot(ms, pick["ia"][:n], label="Park's equations, Runge-Kutta, 5 µs", **style.reference())
    ax1.plot(ms[::every], ia[::every], label="OpenKeito, 2S-DIRK, 100 µs", markersize=2.5,
             **style.series(0, line=False))
    ax1.set_xlabel("Time (ms)")
    ax1.set_ylabel("Current of phase a (pu of the rated peak)")
    style.legend_below(ax1, ncols=1)
    style.panel(ax1, "a")
    for k, h in enumerate(steps):
        rr, kk, nn, pp = waves[("2s-dirk", h)]
        ax2.plot(rr.time[kk:kk + nn] * 1e3, (rr["I(G1.a)"][kk:kk + nn] / I_BASE - pp["ia"][:nn]) * 1e4,
                 label=f"{h * 1e6:g} µs", **style.series(k, marker=False))
    ax2.set_xlabel("Time (ms)")
    ax2.set_ylabel(r"Difference of the current ($10^{-4}$ pu)")
    style.legend_below(ax2)
    style.panel(ax2, "b")
    lib.save_figure(ctx, fig, "machine_short_circuit")

    out = {"peak of the phase current in the reference (pu)": round(float(np.max(np.abs(ref["ia"]))), 3),
           "(w0 h)^2 at 100 us": lib.sig((W0 * 1e-4) ** 2)}
    for method in methods:
        a, b = errors[(method, steps[0])], errors[(method, steps[1])]
        out[f"{method}: max error of the phase current at 100, 50 us (pu)"] = [
            lib.sig(a["ia"]), lib.sig(b["ia"])]
        out[f"{method}: ratio"] = round(a["ia"] / b["ia"], 2)
        out[f"{method}: max error of the field current at 100, 50 us (pu)"] = [
            lib.sig(a["ifd"]), lib.sig(b["ifd"])]
    out["2s-dirk, 100 us: stage iterations per stage, largest in a stage"] = [round(rows[0][6], 2), rows[0][7]]
    return out
