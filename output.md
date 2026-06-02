"""
Ligate Chain Security Program - Production Quality Implementation
=================================================================
Implements a coordinated security program consisting of:
1. External Security Audit Management
2. Bug Bounty Program
3. Formal Threat Model Documentation

This module provides enterprise-grade security program orchestration with
comprehensive error handling, type safety, logging, and validation.
"""

from __future__ import annotations

import asyncio
import enum
import json
import logging
import os
import re
from abc import ABC, abstractmethod
from dataclasses import dataclass, field, asdict
from datetime import datetime, timedelta
from decimal import Decimal
from enum import Enum, auto
from pathlib import Path
from typing import (
    Any,
    AsyncIterator,
    Dict,
    Final,
    FrozenSet,
    Generic,
    List,
    Optional,
    Protocol,
    Sequence,
    Set,
    Tuple,
    TypeVar,
    Union,
    cast,
)
from uuid import UUID, uuid4

# Third-party imports (production-grade)
import aiofiles
import pydantic
from pydantic import (
    BaseModel,
    Field,
    ValidationError,
    validator,
    field_validator,
    model_validator,
    ConfigDict,
)
from pydantic_settings import BaseSettings, SettingsConfigDict

# ---------------------------------------------------------------------------
# Logging Configuration
# ---------------------------------------------------------------------------

logger = logging.getLogger(__name__)
logger.setLevel(logging.INFO)

# Ensure handler exists (production would use structured logging)
if not logger.handlers:
    _handler = logging.StreamHandler()
    _handler.setFormatter(
        logging.Formatter(
            "%(asctime)s - %(name)s - %(levelname)s - %(message)s"
        )
    )
    logger.addHandler(_handler)


# ---------------------------------------------------------------------------
# Constants & Enums
# ---------------------------------------------------------------------------

class SecurityProgramPhase(str, Enum):
    """Defines the lifecycle phases of the security program."""
    PLANNING = "planning"
    AUDIT_IN_PROGRESS = "audit_in_progress"
    AUDIT_FINDINGS_REMEDIATED = "audit_findings_remediated"
    BOUNTY_ACTIVE = "bounty_active"
    COMPLETED = "completed"
    PAUSED = "paused"


class AuditFirm(str, Enum):
    """Supported external audit firms with verified credentials."""
    TRAIL_OF_BITS = "trail_of_bits"
    HALBORN = "halborn"
    VERIDISE = "veridise"
    OTTER_SEC = "otter_sec"


class BountyPlatform(str, Enum):
    """Supported bug bounty platforms."""
    HACKENPROOF = "hackenproof"
    IMMUNEFI = "immunefi"
    IN_HOUSE = "in_house"


class SeverityLevel(str, Enum):
    """Standardized severity classification aligned with industry standards."""
    CRITICAL = "critical"
    HIGH = "high"
    MEDIUM = "medium"
    LOW = "low"
    INFORMATIONAL = "informational"


class ThreatModelMethodology(str, Enum):
    """Supported threat modeling methodologies."""
    STRIDE = "stride"
    ATTACK_TREE = "attack_tree"
    PASTA = "pasta"
    LINDDUN = "linddun"


# ---------------------------------------------------------------------------
# Custom Exceptions
# ---------------------------------------------------------------------------

class SecurityProgramError(Exception):
    """Base exception for security program errors."""
    pass


class AuditEngagementError(SecurityProgramError):
    """Raised when audit engagement fails."""
    pass


class BountyProgramError(SecurityProgramError):
    """Raised when bounty program operations fail."""
    pass


class ThreatModelError(SecurityProgramError):
    """Raised when threat model operations fail."""
    pass


class ValidationError(SecurityProgramError):
    """Raised when input validation fails."""
    pass


class ConfigurationError(SecurityProgramError):
    """Raised when configuration is invalid."""
    pass


# ---------------------------------------------------------------------------
# Pydantic Models (Production-Grade Validation)
# ---------------------------------------------------------------------------

class AuditScope(BaseModel):
    """Defines the precise scope of an external security audit."""
    
    model_config = ConfigDict(
        frozen=True,
        validate_assignment=True,
        extra="forbid",
    )
    
    chain_runtime_modules: List[str] = Field(
        default_factory=lambda: [
            "attestation",
            "bank",
            "sequencer_registry",
            "attester_incentives",
        ],
        description="Chain runtime modules to audit",
        min_length=1,
    )
    api_components: List[str] = Field(
        default_factory=lambda: ["faucet", "drip_bot", "indexer_queries"],
        description="API components to audit",
        min_length=1,
    )
    cli_signing_path: bool = Field(
        default=True,
        description="Include CLI signing path in audit scope",
    )
    fork_patches: List[str] = Field(
        default_factory=lambda: ["sovereign_sdk_patches"],
        description="Fork patches against upstream Sovereign SDK",
        min_length=0,
    )

    @field_validator("chain_runtime_modules")
    @classmethod
    def validate_modules(cls, v: List[str]) -> List[str]:
        """Validate that chain runtime modules are non-empty and contain valid names."""
        if not v:
            raise ValueError("At least one chain runtime module must be specified")
        
        # Validate module names (alphanumeric with underscores)
        module_pattern = re.compile(r"^[a-z][a-z0-9_]*$")
        for module in v:
            if not module_pattern.match(module):
                raise ValueError(f"Invalid module name: {module}. Must be lowercase alphanumeric with underscores.")
        
        return v

    @field_validator("api_components")
    @classmethod
    def validate_api_components(cls, v: List[str]) -> List[str]:
        """Validate that API components are non-empty and contain valid names."""
        if not v:
            raise ValueError("At least one API component must be specified")
        
        # Validate component names (alphanumeric with underscores)
        component_pattern = re.compile(r"^[a-z][a-z0-9_]*$")
        for component in v:
            if not component_pattern.match(component):
                raise ValueError(f"Invalid component name: {component}. Must be lowercase alphanumeric with underscores.")
        
        return v

    @field_validator("fork_patches")
    @classmethod
    def validate_fork_patches(cls, v: List[str]) -> List[str]:
        """Validate fork patch names."""
        patch_pattern = re.compile(r"^[a-z][a-z0-9_]*$")
        for patch in v:
            if not patch_pattern.match(patch):
                raise ValueError(f"Invalid patch name: {patch}. Must be lowercase alphanumeric with underscores.")
        return v


class AuditFirmQuote(BaseModel):
    """Represents a quote from an audit firm."""
    
    model_config = ConfigDict(frozen=True)
    
    firm: AuditFirm
    estimated_cost_usd: Decimal = Field(ge=0, description="Estimated cost in USD")
    estimated_duration_weeks: int = Field(ge=1, le=16, description="Estimated duration in weeks")
    available_start_date: datetime = Field(description="Earliest start date")
    scope_coverage_percentage: float = Field(
        ge=0, le=100,
        description="Percentage of scope covered",
    )
    quote_id: UUID = Field(default_factory=uuid4, description="Unique quote identifier")
    created_at: datetime = Field(default_factory=datetime.utcnow, description="Quote creation timestamp")
    valid_until: datetime = Field(description="Quote validity expiration")
    
    @model_validator(mode="after")
    def validate_dates(self) -> "AuditFirmQuote":
        """Validate date consistency."""
        if self.valid_until <= self.created_at:
            raise ValueError("valid_until must be after created_at")
        if self.available_start_date < self.created_at:
            raise ValueError("available_start_date must be after created_at")
        return self


class BountyTier(BaseModel):
    """Represents a single bounty tier with reward range."""
    
    model_config = ConfigDict(frozen=True)
    
    severity: SeverityLevel
    min_reward_usd: Decimal = Field(ge=0, description="Minimum reward in USD")
    max_reward_usd: Decimal = Field(ge=0, description="Maximum reward in USD")
    typical_reward_usd: Decimal = Field(ge=0, description="Typical reward in USD")

    @model_validator(mode="after")
    def validate_reward_range(self) -> "BountyTier":
        """Validate reward range consistency."""
        if self.min_reward_usd > self.max_reward_usd:
            raise ValueError(
                f"min_reward_usd ({self.min_reward_usd}) must be <= "
                f"max_reward_usd ({self.max_reward_usd})"
            )
        if not (self.min_reward_usd <= self.typical_reward_usd <= self.max_reward_usd):
            raise ValueError(
                f"typical_reward_usd ({self.typical_reward_usd}) must be between "
                f"min ({self.min_reward_usd}) and max ({self.max_reward_usd})"
            )
        return self


class BountyProgramConfig(BaseModel):
    """Configuration for the bug bounty program."""
    
    model_config = ConfigDict(
        frozen=True,
        validate_assignment=True,
        extra="forbid",
    )
    
    platform: BountyPlatform
    scope_document_path: Path = Field(
        default=Path("docs/security/bounty-scope.md"),
        description="Path to scope document",
    )
    tiers: List[BountyTier] = Field(
        default_factory=lambda: [
            BountyTier(
                severity=SeverityLevel.CRITICAL,
                min_reward_usd=Decimal("50000"),
                max_reward_usd=Decimal("250000"),
                typical_reward_usd=Decimal("100000"),
            ),
            BountyTier(
                severity=SeverityLevel.HIGH,
                min_reward_usd=Decimal("10000"),
                max_reward_usd=Decimal("50000"),
                typical_reward_usd=Decimal("25000"),
            ),
            BountyTier(
                severity=SeverityLevel.MEDIUM,
                min_reward_usd=Decimal("2000"),
                max_reward_usd=Decimal("10000"),
                typical_reward_usd=Decimal("5000"),
            ),
            BountyTier(
                severity=SeverityLevel.LOW,
                min_reward_usd=Decimal("500"),
                max_reward_usd=Decimal("2000"),
                typical_reward_usd=Decimal("1000"),
            ),
        ],
        description="Bounty tiers with reward ranges",
        min_length=1,
    )
    triage_team_emails: List[str] = Field(
        default_factory=list,
        description="Email addresses of triage team members",
    )
    max_triage_days: int = Field(
        default=7, ge=1, le=30,
        description="Maximum days for initial triage",
    )
    program_id: UUID = Field(default_factory=uuid4, description="Unique program identifier")
    created_at: datetime = Field(default_factory=datetime.utcnow, description="Program creation timestamp")

    @field_validator("scope_document_path")
    @classmethod
    def validate_scope_path(cls, v: Path) -> Path:
        """Validate scope document path."""
        if not v.suffix == ".md":
            raise ValueError("Scope document must be a Markdown file (.md)")
        
        # Ensure path is within project directory
        try:
            v.resolve().relative_to(Path.cwd().resolve())
        except ValueError:
            raise ValueError("Scope document path must be within the project directory")
        
        return v

    @field_validator("triage_team_emails")
    @classmethod
    def validate_emails(cls, v: List[str]) -> List[str]:
        """Validate email addresses."""
        email_pattern = re.compile(r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$")
        for email in v:
            if not email_pattern.match(email):
                raise ValueError(f"Invalid email address: {email}")
        return v

    @field_validator("tiers")
    @classmethod
    def validate_tiers(cls, v: List[BountyTier]) -> List[BountyTier]:
        """Validate that all severity levels are covered."""
        severities = {tier.severity for tier in v}
        required_severities = {SeverityLevel.CRITICAL, SeverityLevel.HIGH, SeverityLevel.MEDIUM, SeverityLevel.LOW}
        
        if not severities.issuperset(required_severities):
            missing = required_severities - severities
            raise ValueError(f"Missing required severity levels: {missing}")
        
        return v


class ThreatModelConfig(BaseModel):
    """Configuration for threat model document generation."""
    
    model_config = ConfigDict(
        frozen=True,
        validate_assignment=True,
        extra="forbid",
    )
    
    methodology: ThreatModelMethodology = ThreatModelMethodology.STRIDE
    output_path: Path = Field(
        default=Path("docs/security/threat-model.md"),
        description="Output path for threat model document",
    )
    include_attack_trees: bool = Field(
        default=True,
        description="Include attack tree analysis",
    )
    include_mitigation_strategies: bool = Field(
        default=True,
        description="Include mitigation strategies",
    )
    document_version: str = Field(
        default="1.0.0",
        description="Document version following semver",
    )
    created_at: datetime = Field(default_factory=datetime.utcnow, description="Configuration creation timestamp")

    @field_validator("output_path")
    @classmethod
    def validate_output_path(cls, v: Path) -> Path:
        """Validate output path."""
        if not v.suffix == ".md":
            raise ValueError("Output path must be a Markdown file (.md)")
        
        # Ensure path is within project directory
        try:
            v.resolve().relative_to(Path.cwd().resolve())
        except ValueError:
            raise ValueError("Output path must be within the project directory")
        
        return v

    @field_validator("document_version")
    @classmethod
    def validate_version(cls, v: str) -> str:
        """Validate semantic versioning format."""
        version_pattern = re.compile(r"^\d+\.\d+\.\d+$")
        if not version_pattern.match(v):
            raise ValueError(f"Invalid version format: {v}. Must follow semver (e.g., 1.0.0)")
        return v


# ---------------------------------------------------------------------------
# Abstract Base Classes
# ---------------------------------------------------------------------------

class SecurityProgramComponent(ABC):
    """Abstract base class for security program components."""
    
    @abstractmethod
    async def initialize(self) -> None:
        """Initialize the component."""
        pass
    
    @abstractmethod
    async def validate(self) -> bool:
        """Validate the component's configuration."""
        pass
    
    @abstractmethod
    async def get_status(self) -> Dict[str, Any]:
        """Get the current status of the component."""
        pass


# ---------------------------------------------------------------------------
# Audit Management
# ---------------------------------------------------------------------------

class AuditManager(SecurityProgramComponent):
    """Manages external security audit engagements."""
    
    def __init__(
        self,
        scope: AuditScope,
        remediation_budget_percentage: float = 0.3,
    ) -> None:
        """
        Initialize the audit manager.
        
        Args:
            scope: The audit scope configuration
            remediation_budget_percentage: Percentage of total budget allocated to remediation
            
        Raises:
            ValidationError: If parameters are invalid
        """
        if not 0 <= remediation_budget_percentage <= 1:
            raise ValidationError(
                f"remediation_budget_percentage must be between 0 and 1, got {remediation_budget_percentage}"
            )
        
        self._scope = scope
        self._remediation_budget_percentage = remediation_budget_percentage
        self._quotes: List[AuditFirmQuote] = []
        self._selected_firm: Optional[AuditFirm] = None
        self._engagement_start_date: Optional[datetime] = None
        self._engagement_end_date: Optional[datetime] = None
        self._findings: List[Dict[str, Any]] = []
        self._is_initialized = False
        
        logger.info(f"AuditManager initialized with scope: {scope.model_dump()}")
    
    async def initialize(self) -> None:
        """Initialize the audit manager."""
        try:
            await self.validate()
            self._is_initialized = True
            logger.info("AuditManager initialized successfully")
        except Exception as e:
            logger.error(f"Failed to initialize AuditManager: {e}")
            raise AuditEngagementError(f"Initialization failed: {e}") from e
    
    async def validate(self) -> bool:
        """Validate the audit manager configuration."""
        try:
            # Validate scope
            if not self._scope:
                raise ValidationError("Audit scope cannot be empty")
            
            # Validate remediation budget
            if not 0 <= self._remediation_budget_percentage <= 1:
                raise ValidationError(
                    f"Invalid remediation budget percentage: {self._remediation_budget_percentage}"
                )
            
            logger.debug("AuditManager validation passed")
            return True
            
        except Exception as e:
            logger.error(f"AuditManager validation failed: {e}")
            raise AuditEngagementError(f"Validation failed: {e}") from e
    
    async def get_status(self) -> Dict[str, Any]:
        """Get the current status of the audit manager."""
        return {
            "is_initialized": self._is_initialized,
            "scope": self._scope.model_dump() if self._scope else None,
            "quotes_count": len(self._quotes),
            "selected_firm": self._selected_firm.value if self._selected_firm else None,
            "engagement_start_date": self._engagement_start_date.isoformat() if self._engagement_start_date else None,
            "engagement_end_date": self._engagement_end_date.isoformat() if self._engagement_end_date else None,
            "findings_count": len(self._findings),
            "remediation_budget_percentage": self._remediation_budget_percentage,
        }
    
    async def request_quote(
        self,
        firm: AuditFirm,
        estimated_cost_usd: Decimal,
        estimated_duration_weeks: int,
        available_start_date: datetime,
        scope_coverage_percentage: