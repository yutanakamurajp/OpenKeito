"""The power system stabilizer of ``add_machine_controls``.

A machine against an infinite bus, with the excitation system and with or
without the stabilizer, is compared with the eigenvalues of the same system
linearized here, independently of the solver: Park's equations with the
stator and network transients neglected (the model of small-signal studies:
Kundur, "Power System Stability and Control", chapter 12), the regulator and
the stabilizer, differentiated numerically at the operating point. The
stabilizer alone is compared with the closed form of its step response.
Each tolerance is derived where it is used.
"""

from __future__ import annotations

import cmath
import math

import numpy as np
import pytest

import openkeito
from openkeito.convert import Conversion, synchronous_machine
from openkeito.transition import EXCITER, PSS, PSS_MINIMUM_ACTIVE_POWER_PU, add_machine_controls

F0 = 60.0
W0 = 2 * math.pi * F0
#: The machine: the typical round-rotor unit of the converters with H = 3.5 s,
#: 555 MVA at 24 kV, delivering 0.9 + j0.3 pu at 1 pu through 0.02 + j0.65 pu
#: to an infinite bus: a long line and a high regulator gain, where the
#: regulator takes away the damping of the swing (Kundur, chapter 12).
RATING_MVA, RATED_KV = 555.0, 24.0
S_PU = complex(0.9, 0.3)
LINE_PU = complex(0.02, 0.65)
INERTIA_S = 3.5


# The linearized system ------------------------------------------------------------


class Reference:
    """d(state)/dt of the machine against the infinite bus in the frame of
    the rotor, with the states [delta, speed, psi_dT, psi_qT, psi_fd, psi_1d,
    psi_1q, psi_2q, Efd, washout, lead 1, lead 2] (the last three stay 0
    without a stabilizer). psi_dT and psi_qT are the stator fluxes plus those
    of the line, psi_T = psi - X i: the line carries the stator current, so
    the stator and the line together are

        E_d = (1/w0) d(psi_dT)/dt - w psi_qT - (Ra + R) i_d
        E_q = (1/w0) d(psi_qT)/dt + w psi_dT - (Ra + R) i_q

    with the bus voltage E_d + j E_q = E (sin d + j cos d), d the angle of
    the q axis against the bus (Kundur, sections 3.3 and 12.3)."""

    N = 12

    def __init__(self, machine: dict, exciter: dict, pss: dict | None, s_pu: complex = S_PU,
                 line_pu: complex = LINE_PU):
        m = machine
        self.line = line_pu
        xl = m["xl_pu"]
        self.lad, self.laq = m["xd_pu"] - xl, m["xq_pu"] - xl
        self.ld, self.lq, self.ra, self.h = m["xd_pu"], m["xq_pu"], m["ra_pu"], m["inertia_h_s"]
        # The windings from the standard parameters (docs/numerical-method.md,
        # "From standard to fundamental parameters"; Kundur, section 4.2).
        lad, laq = self.lad, self.laq
        lfd = lad * (m["xd_transient_pu"] - xl) / (m["xd_pu"] - m["xd_transient_pu"])
        l1d = ((m["xd_transient_pu"] - xl) * (m["xd_subtransient_pu"] - xl)
               / (m["xd_transient_pu"] - m["xd_subtransient_pu"]))
        l1q = laq * (m["xq_transient_pu"] - xl) / (m["xq_pu"] - m["xq_transient_pu"])
        l2q = ((m["xq_transient_pu"] - xl) * (m["xq_subtransient_pu"] - xl)
               / (m["xq_transient_pu"] - m["xq_subtransient_pu"]))
        self.rfd = (lad + lfd) / (W0 * m["td0_transient_s"])
        self.r1d = (l1d + lad * lfd / (lad + lfd)) / (W0 * m["td0_subtransient_s"])
        self.r1q = (laq + l1q) / (W0 * m["tq0_transient_s"])
        self.r2q = (l2q + laq * l1q / (laq + l1q)) / (W0 * m["tq0_subtransient_s"])
        self.exciter, self.pss = exciter, pss
        # Fluxes [dT, qT, fd, 1d, 1q, 2q] from the currents [d, q, fd, 1d, 1q, 2q].
        xe = line_pu.imag
        self.inductance = np.array([
            [-(self.ld + xe), 0, lad, lad, 0, 0],
            [0, -(self.lq + xe), 0, 0, laq, laq],
            [-lad, 0, lad + lfd, lad, 0, 0],
            [-lad, 0, lad, lad + l1d, 0, 0],
            [0, -laq, 0, 0, laq + l1q, laq],
            [0, -laq, 0, 0, laq, laq + l2q]])
        self.to_currents = np.linalg.inv(self.inductance)

        # The operating point (Kundur, section 3.6): terminal voltage 1 at
        # angle 0, the bus behind the line, the q axis on V + (Ra + j Xq) I.
        v = 1.0
        i = (s_pu / v).conjugate()
        bus = v - line_pu * i
        self.bus_voltage, self.bus_angle = abs(bus), cmath.phase(bus)
        delta = cmath.phase(v + complex(self.ra, self.lq) * i)
        turn = cmath.exp(-1j * (delta - math.pi / 2))
        vq = (v * turn).imag
        i_d, i_q = (i * turn).real, (i * turn).imag
        ifd = (vq + self.ra * i_q + self.ld * i_d) / lad
        fluxes = self.inductance @ np.array([i_d, i_q, ifd, 0.0, 0.0, 0.0])
        self.torque = fluxes[0] * i_q - fluxes[1] * i_d
        efd = lad * ifd
        self.vref = v + efd / exciter["gain"]
        self.x0 = np.concatenate(([delta, 1.0], fluxes, [efd, 0.0, 0.0, 0.0]))

    def derivative(self, x: np.ndarray, dvref: float = 0.0) -> np.ndarray:
        delta, w = x[0] - self.bus_angle, x[1]
        r, xe = self.ra + self.line.real, self.line.imag
        c = self.to_currents @ x[2:8]
        dx = np.zeros(self.N)
        dx[0] = W0 * (w - 1.0)
        dx[1] = (self.torque - (x[2] * c[1] - x[3] * c[0])) / (2 * self.h)
        dx[2] = W0 * (self.bus_voltage * math.sin(delta) + r * c[0] + w * x[3])
        dx[3] = W0 * (self.bus_voltage * math.cos(delta) + r * c[1] - w * x[2])
        dx[4] = W0 * self.rfd * (x[8] / self.lad - c[2])
        dx[5] = -W0 * self.r1d * c[3]
        dx[6] = -W0 * self.r1q * c[4]
        dx[7] = -W0 * self.r2q * c[5]
        # The terminal voltage: that of the bus plus the drop of the line,
        # with the derivatives of the currents that those of the fluxes give.
        dc = self.to_currents @ dx[2:8]
        vd = self.bus_voltage * math.sin(delta) + self.line.real * c[0] - w * xe * c[1] + xe / W0 * dc[0]
        vq = self.bus_voltage * math.cos(delta) + self.line.real * c[1] + w * xe * c[0] + xe / W0 * dc[1]
        vs = 0.0
        if self.pss is not None:
            p = self.pss
            # Washout, then the lead-lag stages (state: the lag of each).
            vs = p["gain"] * (w - 1.0) - x[9]
            dx[9] = vs / p["washout_time_constant_s"]
            for k, n in ((10, "1"), (11, "2")):
                lead, lag = p[f"lead_time_constant_{n}_s"], p[f"lag_time_constant_{n}_s"]
                if lag > 0:
                    dx[k] = (vs - x[k]) / lag
                    vs = lead / lag * vs + (1 - lead / lag) * x[k]
        ex = self.exciter
        dx[8] = (ex["gain"] * (self.vref + dvref + vs - math.hypot(vd, vq)) - x[8]) / ex["time_constant_s"]
        return dx

    def linearized(self) -> tuple[np.ndarray, np.ndarray]:
        """(A, B) of d(dx)/dt = A dx + B dvref, by central differences (the
        equations are smooth: the error is of the order 1e-12 / 1e-6)."""
        a = np.zeros((self.N, self.N))
        for k in range(self.N):
            e = np.zeros(self.N)
            e[k] = 1e-6
            a[:, k] = (self.derivative(self.x0 + e) - self.derivative(self.x0 - e)) / 2e-6
        b = (self.derivative(self.x0, 1e-6) - self.derivative(self.x0, -1e-6)) / 2e-6
        return a, b

    def swing_mode(self) -> complex:
        """The eigenvalue of the swing of the rotor: of the oscillatory ones
        below the stator's (which is near the rated frequency), the one in
        which the angle takes the largest part."""
        a, _ = self.linearized()
        values, vectors = np.linalg.eig(a)
        parts = [abs(vectors[0, k]) / np.linalg.norm(vectors[:, k]) if 1.0 < values[k].imag < 0.5 * W0 else 0.0
                 for k in range(len(values))]
        return complex(values[int(np.argmax(parts))])

    def step_response(self, t: np.ndarray, dvref: float) -> np.ndarray:
        """Speed deviation after a step of the voltage reference:
        dx = sum_k v_k (e^{s_k t} - 1) / s_k (V^-1 B)_k dvref."""
        a, b = self.linearized()
        values, vectors = np.linalg.eig(a)
        weights = np.linalg.solve(vectors, b.astype(complex)) * dvref
        y = np.zeros(t.shape, dtype=complex)
        for k in np.flatnonzero(np.abs(values) > 1e-9):  # the unused stabilizer states are 0
            y += vectors[1, k] * weights[k] * (np.exp(values[k] * t) - 1.0) / values[k]
        return y.real


# The same system in the solver ----------------------------------------------------


def infinite_bus(pss, *, time_step_s: float, end_time_s: float, dvref: float, step_time_s: float = 0.2):
    """(model, machine component) of the machine, the line and the bus."""
    machine, _ = synchronous_machine("G1", "b1", frequency_hz=F0, rated_power_mva=RATING_MVA,
                                     rated_voltage_kv=RATED_KV, s_mva=S_PU * RATING_MVA, voltage_pu=1.0,
                                     angle_deg=0.0, parameters={"inertia_h_s": INERTIA_S})
    z_base = RATED_KV ** 2 / RATING_MVA
    bus = 1.0 - LINE_PU * (S_PU / 1.0).conjugate()
    model = {
        "format": "openkeito-model", "version": "0.5", "name": "machine against an infinite bus",
        "simulation": {"time_step_s": time_step_s, "end_time_s": end_time_s, "initialization": "steady_state",
                       "output_interval_steps": round(1e-3 / time_step_s)},
        "components": [
            machine,
            {"type": "coupled_rl", "name": "LINE", "buses": ["b1", "inf"],
             "impedance": {"type": "sequence", "r1_ohm": LINE_PU.real * z_base, "l1_h": LINE_PU.imag * z_base / W0,
                           "r0_ohm": LINE_PU.real * z_base, "l0_h": 3 * LINE_PU.imag * z_base / W0}},
            {"type": "three_phase_source", "name": "INF", "bus": "inf", "frequency_hz": F0,
             "line_voltage_rms_v": abs(bus) * RATED_KV * 1e3, "angle_deg": math.degrees(cmath.phase(bus))}],
        "controls": [], "outputs": [],
    }
    conv = Conversion(model=model, mapping={"elements": [], "buses": {}})
    add_machine_controls(conv, governor=False, pss=pss)
    block = next(b for b in model["controls"] if b["name"] == "G1_dvref")
    block.clear()
    block.update({"type": "step", "name": "G1_dvref", "time_s": step_time_s, "before": 0.0, "after": dvref})
    if not pss:
        model["controls"].append({"type": "measurement", "name": "G1_speed", "quantity": {
            "type": "component_signal", "component": "G1", "signal": "speed_pu"}})
    model["outputs"] = [{"type": "signal", "signal": s} for s in ("G1_speed", "G1_vt", "G1_efd")]
    if pss:
        model["outputs"].append({"type": "signal", "signal": "G1_pss"})
    return model, machine


def modes(y: np.ndarray, dt: float, order: int = 8) -> np.ndarray:
    """The exponents s_k of y(t) = sum_k c_k e^{s_k t} sampled every dt, by
    the matrix pencil method (Hua and Sarkar, IEEE Trans. Acoustics, Speech,
    and Signal Processing, 1990): the eigenvalues of pinv(Y0) Y1 for the
    shifted Hankel matrices of the samples, on the ``order`` largest singular
    directions."""
    n = len(y)
    width = n // 3
    hankel = np.array([y[k:k + width + 1] for k in range(n - width)])
    _, _, vh = np.linalg.svd(hankel, full_matrices=False)
    v = vh[:order].conj().T
    z = np.linalg.eigvals(np.linalg.pinv(v[:-1]) @ v[1:])
    return np.log(z.astype(complex)) / dt


def the_machine() -> dict:
    return synchronous_machine("G1", "b1", frequency_hz=F0, rated_power_mva=RATING_MVA, rated_voltage_kv=RATED_KV,
                               s_mva=S_PU * RATING_MVA, voltage_pu=1.0, angle_deg=0.0,
                               parameters={"inertia_h_s": INERTIA_S})[0]


@pytest.mark.parametrize("pss", [False, True])
def test_swing_against_an_infinite_bus_has_the_eigenvalue_of_the_linearized_system(pss):
    """A small step of the voltage reference starts the swing of the rotor
    against the bus. Its exponent (decay and frequency), read from the speed
    of the run, is the eigenvalue of the system linearized at the operating
    point. With the regulator alone the swing grows (the regulator of gain
    200 takes away more damping than the rotor windings give: Kundur,
    chapter 12); the stabilizer gives it a damping ratio of 0.17."""
    reference = Reference(the_machine(), EXCITER, PSS if pss else None)
    # The operating point is a steady state of the reference.
    assert np.max(np.abs(reference.derivative(reference.x0))) < 1e-9
    expected = reference.swing_mode()
    if pss:
        assert expected.real == pytest.approx(-1.206, abs=2e-3) and expected.imag == pytest.approx(6.912, abs=2e-3)
        assert -expected.real / abs(expected) == pytest.approx(0.172, abs=1e-3)
    else:
        assert expected.real == pytest.approx(0.601, abs=2e-3) and expected.imag == pytest.approx(7.358, abs=2e-3)

    # Without the stabilizer the swing grows by e^(0.6 * 4) = 11 over the
    # run: a smaller step keeps it small (the speed stays within 7e-5 pu).
    time_step_s, t0, dvref = 1e-4, 0.2, 1e-3 if pss else 2e-4
    model, _ = infinite_bus(pss, time_step_s=time_step_s, end_time_s=4.2, dvref=dvref, step_time_s=t0)
    result = openkeito.run(model)
    t, dw = result.time, result["S(G1_speed)"] - 1.0
    # Until the step nothing moves but for the integration error of the
    # start, far below (w0 h)^2 = 1.4e-3 (measured 1e-6 pu of speed); the
    # stabilizer is at rest.
    assert np.max(np.abs(dw[t < t0])) < 5e-6
    if pss:
        assert np.max(np.abs(result["S(G1_pss)"][t < t0])) < PSS["gain"] * 5 * 5e-6
    after = t >= t0 - 1e-9
    found = modes(dw[after], 1e-3)
    measured = complex(found[np.argmin(np.abs(found - expected))])
    print(f"swing with pss={pss}: linearized {expected:.4f}, run {measured:.4f} 1/s; damping ratio "
          f"{-expected.real / abs(expected):.4f} and {-measured.real / abs(measured):.4f}")
    # What separates the two:
    # - The step moves the operating point, and the eigenvalue with it:
    #   measured with steps of +0.01 and -0.01, by -+0.021 1/s in the real
    #   part and +-0.5 % in the frequency, so 2e-3 1/s and 0.05 % for 1e-3
    #   (and a fifth of that for 2e-4).
    # - The time step: the trapezoidal rule lowers a frequency by (w h)^2/12
    #   (5e-8 here); the delay of one step between the controls and the
    #   circuit turns the loop by w h = 7e-4 rad.
    # Measured: 4.7e-4 of the frequency and 2.1e-3 1/s with the stabilizer,
    # 1.0e-4 and 2.3e-4 1/s without; the same at 50 us.
    assert measured.imag == pytest.approx(expected.imag, rel=1.5e-3)
    assert measured.real == pytest.approx(expected.real, abs=6e-3)
    zeta, zeta_expected = -measured.real / abs(measured), -expected.real / abs(expected)
    assert zeta == pytest.approx(zeta_expected, abs=1e-3)
    assert (zeta > 0.15) if pss else (zeta < -0.07)
    if pss:
        # The whole response of the speed is that of the linearized system:
        # within 5 % of its peak (measured 2.3 %: the error of the start,
        # 7e-7 pu, and the moved operating point).
        response = reference.step_response(t[after] - t0, dvref)
        peak = float(np.max(np.abs(response)))
        assert peak == pytest.approx(3.03e-5, rel=0.01)
        assert np.max(np.abs(dw[after] - response)) < 0.05 * peak


def test_a_stabilizer_needs_load_to_act():
    """Why a machine without active power gets no stabilizer unless it is
    asked for (``PSS_MINIMUM_ACTIVE_POWER_PU``): the stabilizer reaches the
    torque through the flux, in proportion to the load. In the linearized
    system it moves the decay of the swing from -0.17 to -1.85 1/s at half
    load, from -0.71 to -1.08 1/s at a tenth, and by 0.03 1/s at none,
    whether the machine delivers or absorbs reactive power."""
    def decay(s_pu, pss):
        return Reference(the_machine(), EXCITER, pss, s_pu=s_pu).swing_mode().real

    assert decay(0.5, None) == pytest.approx(-0.17, abs=0.02) and decay(0.5, PSS) == pytest.approx(-1.85, abs=0.02)
    assert decay(PSS_MINIMUM_ACTIVE_POWER_PU, PSS) - decay(PSS_MINIMUM_ACTIVE_POWER_PU, None) < -0.25
    for s_pu in (0.0, 0.3j, -0.3j):
        assert abs(decay(s_pu, PSS) - decay(s_pu, None)) < 0.05 and decay(s_pu, PSS) < -0.4


# The stabilizer alone -------------------------------------------------------------


def stabilizer_step(t: np.ndarray, dw: float, p: dict) -> np.ndarray:
    """Output of the stabilizer (before its limits) for a step dw of the
    speed: Y(s) = dw K TW (1 + s T1)(1 + s T3) / ((1 + s TW)(1 + s T2)
    (1 + s T4)), by partial fractions (simple poles)."""
    numerator = np.poly1d([p["gain"] * p["washout_time_constant_s"]])
    denominator = np.poly1d([p["washout_time_constant_s"], 1.0])
    for k in ("1", "2"):
        if p[f"lag_time_constant_{k}_s"] > 0:
            numerator = numerator * np.poly1d([p[f"lead_time_constant_{k}_s"], 1.0])
            denominator = denominator * np.poly1d([p[f"lag_time_constant_{k}_s"], 1.0])
    y = np.zeros(t.shape, dtype=complex)
    for pole in denominator.roots:
        y += numerator(pole) / denominator.deriv()(pole) * np.exp(pole * t)
    return dw * y.real


#: Two stages with separate time constants: the stabilizer of the two-area
#: system of Kundur, chapter 12 (gain 20, washout 10 s, 0.05/0.02 s and
#: 3.0/5.4 s).
TWO_STAGES = {"gain": 20.0, "washout_time_constant_s": 10.0, "lead_time_constant_1_s": 0.05,
              "lag_time_constant_1_s": 0.02, "lead_time_constant_2_s": 3.0, "lag_time_constant_2_s": 5.4}


@pytest.mark.parametrize("parameters, dw", [(TWO_STAGES, 1e-3), (PSS, 1e-3), (PSS, 5e-3)])
def test_stabilizer_step_response_against_the_transfer_function(parameters, dw):
    """The blocks of the stabilizer, fed with a step of the speed in place of
    the speed of the machine, give the step response of gain * washout *
    lead-lag stages, held within the limits."""
    time_step_s, t0 = 1e-4, 0.1
    model, _ = infinite_bus(parameters, time_step_s=time_step_s, end_time_s=3.0, dvref=0.0)
    model["simulation"]["output_interval_steps"] = 1
    speed = next(b for b in model["controls"] if b["name"] == "G1_speed")
    speed.clear()
    speed.update({"type": "step", "name": "G1_speed", "time_s": t0, "before": 1.0, "after": 1.0 + dw})
    result = openkeito.run(model)
    t, y = result.time, result["S(G1_pss)"]
    p = {**PSS, **parameters}
    assert np.all(y[t < t0 - 1e-9] == 0.0)
    # A block integrates its input along the straight line between two grid
    # points: the step that the input makes at the grid point t0 is, for the
    # blocks, a ramp over the step before it, whose response is that of a
    # step at t0 - h/2 to second order in h / T.
    after = t >= t0 - 1e-9
    unlimited = stabilizer_step(t[after] - t0 + time_step_s / 2, dw, p)
    expected = np.clip(unlimited, p["minimum_pu"], p["maximum_pu"])
    peak = float(np.max(np.abs(unlimited)))
    limited = peak > p["maximum_pu"]
    assert limited == (dw > 2e-3)
    if limited:
        assert y.max() == p["maximum_pu"] and np.count_nonzero(y == p["maximum_pu"]) > 100
    error = float(np.max(np.abs(y[after] - expected)))
    print(f"stabilizer step: {error:.3g} of a peak of {peak:.3g}")
    # Second order in h / T of the shortest time constant (0.02 s and
    # 0.033 s; h / T = 5e-3 and 3e-3): the trapezoidal rule is off by at most
    # (h/T)^2 / (12 e) of the step of a mode (docs/numerical-method.md,
    # "Blocks with a state"), and the ramp in place of the step by about
    # (h/T)^2 / 24. Measured: 0.08 and 0.10 (h/T)^2 of the peak (5.2e-8 of
    # 0.028 and 4.1e-8 of 0.044).
    shortest = p["lag_time_constant_1_s"]
    assert error < 0.25 * (time_step_s / shortest) ** 2 * peak


# What add_machine_controls takes --------------------------------------------------


def test_stabilizer_options():
    def controls(active_power_pu=None, **options):
        machine = the_machine()
        if active_power_pu is not None:
            machine["operating_point"]["active_power_mw"] = active_power_pu * RATING_MVA
        conv = Conversion(model={"version": "0.5", "components": [machine], "controls": []},
                          mapping={"elements": [{"component": "G1", "element": "gen/0"}], "buses": {}})
        done = add_machine_controls(conv, **options)
        warnings[:] = [w for w in conv.warnings if "stabilizer" in w]
        return done, {b["name"]: b for b in conv.model["controls"]}, machine

    warnings: list[str] = []

    # None unless asked for: the callers of before get what they got.
    done, blocks, _ = controls()
    assert done["G1"]["pss"] is None and not any("pss" in name for name in blocks)
    assert blocks["G1_v_error"]["inputs"] == ["G1_vref", "G1_vt"]
    # The typical values, with one stage.
    done, blocks, _ = controls(pss=True)
    assert done["G1"]["pss"] == PSS
    assert blocks["G1_v_error"] == {"type": "sum", "name": "G1_v_error", "inputs": ["G1_vref", "G1_vt", "G1_pss"],
                                    "signs": [1.0, -1.0, 1.0]}
    assert blocks["G1_pss_washout"]["numerator"] == [9.5 * 1.4, 0.0]
    assert blocks["G1_pss_washout"]["denominator"] == [1.4, 1.0]
    assert blocks["G1_pss_lead1"]["lead_time_constant_s"] == 0.154 and "G1_pss_lead2" not in blocks
    assert blocks["G1_pss"]["input"] == "G1_pss_lead1"
    assert (blocks["G1_pss"]["minimum"], blocks["G1_pss"]["maximum"]) == (-0.1, 0.1)
    # Values of one machine, by its element; without a governor.
    done, blocks, _ = controls(governor=False, machines={"gen/0": {"pss": {**TWO_STAGES, "maximum_pu": 0.2}}})
    assert done["G1"]["governor"] is None and done["G1"]["pss"]["gain"] == 20.0
    assert blocks["G1_pss"]["input"] == "G1_pss_lead2" and blocks["G1_pss"]["maximum"] == 0.2
    assert blocks["G1_pss_lead2"]["input"] == "G1_pss_lead1" and "G1_speed" in blocks
    done, blocks, _ = controls(pss=True, machines={"G1": {"pss": False}})
    assert done["G1"]["pss"] is None and "G1_pss" not in blocks
    # A stabilizer acts through the excitation system: none without one.
    done, blocks, machine = controls(exciter=False, pss=True)
    assert done["G1"]["pss"] is None and "G1_pss" not in blocks and "field_voltage_signal" not in machine
    # Nor for a machine without active power (a synchronous condenser),
    # which a warning names, unless it is asked for by name.
    assert warnings == []
    for power in (0.0, -0.05, 0.099):
        done, blocks, _ = controls(power, pss=True)
        assert done["G1"]["pss"] is None and "G1_pss" not in blocks and blocks["G1_efd"]["gain"] == 200.0
        assert len(warnings) == 1 and "no stabilizer" in warnings[0] and warnings[0].endswith(": gen/0")
    done, blocks, _ = controls(0.0, pss=True, machines={"gen/0": {"pss": True}})
    assert done["G1"]["pss"] == PSS and "G1_pss" in blocks and warnings == []
    done, blocks, _ = controls(-0.5, pss=True)
    assert done["G1"]["pss"] == PSS and warnings == []
    # What cannot be built.
    with pytest.raises(ValueError, match="pss has no parameter kstab"):
        controls(pss={"kstab": 20.0})
    with pytest.raises(ValueError, match="washout_time_constant_s"):
        controls(pss={"washout_time_constant_s": 0.0})
    with pytest.raises(ValueError, match="stage 2"):
        controls(pss={"lead_time_constant_2_s": 0.1})
    with pytest.raises(ValueError, match="limits"):
        controls(pss={"minimum_pu": 0.05})


def test_models_with_a_stabilizer_are_valid():
    for parameters in (True, TWO_STAGES):
        model, _ = infinite_bus(parameters, time_step_s=1e-4, end_time_s=0.01, dvref=1e-3)
        report = openkeito.validate(model)
        assert report["valid"], report
